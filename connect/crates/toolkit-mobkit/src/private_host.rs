//! A process-local Toolkit control channel, separate from every public RPC door.
//!
//! Only the trusted creator of a local child passes these local IPC endpoints.
//! Descriptor numbers, JSON fields and a remote SDK are not authentication. This
//! protects against untrusted model/RPC callers, not native code execution by
//! the same OS user or code already running inside the trusted host process.

use crate::installed_host::operator_setup::OperatorParentOutcome;
use crate::private_originals::{
    PrivateAuthenticatedEvent, PrivateOriginalCancellation, PrivateOriginalError,
    PrivateOriginalRef, PrivateOriginalRetention, PrivateOriginals, PrivateServiceAccessDecision,
    PrivateServiceGrantObservation, PrivateOriginalSubmit, PrivateOriginalSubmission,
    PrivateServiceInputObservation,
};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_toolkit::host_lifetime::control::{
    HostBootstrapSelection, HostCloseHandle, HostCloseObservation, HostCloseReason, HostCloseState,
    HostComponentObservation, HostLifetimeError, PrivateHostLifetimeControl, SelectedHostArtifact,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io,
    os::fd::{AsRawFd, OwnedFd, RawFd},
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    sync::Arc,
};
use tokio::{
    io::{
        AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt,
        BufReader,
    },
    sync::{mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
};
use toolkit_package::Id;

mod console_login;
pub use console_login::{ConsoleLoginHandle, ConsoleLoginRefusal, ConsoleLoginReservation,
    ConsoleLoginRetirement, ConsoleLoginSelection, ConsoleLogins};
mod service_parent;
pub use service_parent::{ServiceParentEndpoint, ServiceParentCommand, ServiceParentResponse,
    ServiceParentAdministrationRequest, ServiceParentDecision, ServiceParentDecisionSource};
mod startup;
pub use startup::{ApplicationUpdateStartup, PrivateHostStartup, PrivateHostStartupIntent};

pub const PRIVATE_HOST_PROTOCOL_VERSION: u16 = 1;
// Shared with both SDKs; the fixed private frame bound includes its newline.
const MAX_FRAME_BYTES: u64 = 1_048_576;
const MAX_PENDING_JOINS: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum PrivateHostError {
    #[error("private host transport is unavailable")]
    Io(#[from] io::Error),
    #[error("private host protocol was rejected")]
    Protocol,
    #[error("private host owner refused the operation")]
    Owner(#[from] HostLifetimeError),
    #[error("private host owner did not settle")]
    NotSettled,
    #[error("private host owner task failed")]
    OwnerTask,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u16,
    id: u64,
    command: Command,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    PrepareApplication { plan: SelectedHostArtifact },
    ReplaceApplication { startup: ApplicationUpdateStartup },
    Bootstrap {
        selection: HostBootstrapSelection,
    },
    EnrollConsoleLogin { login: ConsoleLoginSelection },
    RetireConsoleLogin { enrollment_id: u64 },
    OpenComponent {
        component: Id,
    },
    BeginCloseComponent {
        component: Id,
        reason: HostCloseReason,
    },
    BeginCloseActivation {
        reason: HostCloseReason,
    },
    ObserveClose {
        handle: HostCloseHandle,
    },
    JoinClose {
        handle: HostCloseHandle,
    },
    ReviewControllerParent {
        submission: PrivateOriginalSubmit,
    },
    ReviewSetupRequest { submission: PrivateOriginalSubmit },
    ReviewRemoteSetupRequest { submission: PrivateOriginalSubmit, login: RemoteControllerLogin },
    ReviewRemoteControllerParent {
        submission: PrivateOriginalSubmit,
        login: RemoteControllerLogin,
    },
    ObserveControllerReview { review_id: u64 },
    AnswerControllerReview {
        review_id: u64,
        approval_id: String,
        decision: RemoteReviewDecision,
    },
    RetireControllerReview { review_id: u64 },
    NextServiceParentAdministration,
    SubmitServiceParentAdministration { exchange_id: u64, proposal_sha256: String, decision: ServiceParentDecision },
    ResultServiceParentAdministration { exchange_id: u64 },
    RetireServiceParentAdministration { exchange_id: u64 },
    SubmitAuthenticatedOriginal {
        submission: PrivateOriginalSubmit,
    },
    RetainAuthenticatedEvent {
        event: PrivateAuthenticatedEvent,
    },
    CancelOriginalOperation {
        original: PrivateOriginalRef,
    },
    ObserveServiceInputReceipt { original: PrivateOriginalRef },
    ApplyServiceAccessDecision {
        decision: PrivateServiceAccessDecision,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Response {
    version: u16,
    id: u64,
    result: Reply,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    ApplicationPrepared { manifest: SelectedHostArtifact },
    BootstrapAccepted,
    ConsoleLoginEnrolled { enrollment_id: u64 },
    ConsoleLoginRetired { enrollment_id: u64, observation: ConsoleLoginRetirement },
    ControllerParentReviewed {
        outcome: OperatorParentOutcome,
    },
    SetupRequestReviewed { outcome: OperatorParentOutcome },
    ControllerReview { review_id: u64, status: RemoteReviewStatus },
    ServiceParentAdministrationRequest { exchange_id: u64, proposal_sha256: String,
        proposal: crate::private_originals::ServiceParentAuthorizationProposal },
    ServiceParentAdministrationSubmitted { exchange_id: u64 },
    ServiceParentAdministrationResult { exchange_id: u64, observation: crate::private_originals::ServiceParentOperationObservation },
    ServiceParentAdministrationRetired { exchange_id: u64, observation: crate::private_originals::ServiceParentOperationObservation },
    ServiceAccessDecisionApplied {
        observation: PrivateServiceGrantObservation,
    },
    OriginalSubmitted {
        original: PrivateOriginalRef,
        outcome: PrivateOriginalSubmission,
    },
    AuthenticatedEventRetained {
        original: PrivateOriginalRef,
        state: PrivateOriginalRetention,
    },
    OriginalOperationCancelled {
        original: PrivateOriginalRef,
        state: PrivateOriginalCancellation,
    },
    ServiceInputReceipt { original: PrivateOriginalRef, observation: PrivateServiceInputObservation },
    Component {
        observation: HostComponentObservation,
    },
    Close {
        observation: HostCloseObservation,
    },
    Error {
        error: WireError,
    },
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireError {
    SourceConflict,
    InvalidSelection,
    Unavailable,
    Denied,
    WrongOwner,
    NotSettled,
}

impl From<HostLifetimeError> for WireError {
    fn from(error: HostLifetimeError) -> Self {
        match error {
            HostLifetimeError::InvalidSelection => Self::InvalidSelection,
            HostLifetimeError::Unavailable => Self::Unavailable,
            HostLifetimeError::Denied => Self::Denied,
            HostLifetimeError::WrongOwner => Self::WrongOwner,
            HostLifetimeError::NotSettled => Self::NotSettled,
        }
    }
}

fn original_response(id: u64, result: Result<Reply, PrivateOriginalError>) -> Response {
    Response {
        version: PRIVATE_HOST_PROTOCOL_VERSION,
        id,
        result: result.unwrap_or_else(|error| Reply::Error {
            error: match error {
                PrivateOriginalError::Unavailable => WireError::Unavailable,
                PrivateOriginalError::Conflict => WireError::SourceConflict,
                PrivateOriginalError::Denied => WireError::Denied,
            },
        }),
    }
}

fn service_input_response(id: u64, result: Result<Reply, PrivateOriginalError>) -> Response {
    let reply = original_response(id, result);
    // The existing SDK receipt profile accepts finite numbers, safe integers
    // and at most 128 nested containers, including this response envelope.
    // An unsupported native payload is an unavailable read, not a rewritten
    // completion or a reason to tear down unrelated private-channel custody.
    if private_reply_fits(&reply) {
        reply
    } else {
        original_response(id, Err(PrivateOriginalError::Unavailable))
    }
}

fn private_reply_fits(reply: &Response) -> bool {
    serde_json::to_vec(reply).is_ok_and(|bytes| bytes.len() < MAX_FRAME_BYTES as usize)
        && serde_json::to_value(reply).is_ok_and(|value| service_receipt_wire_value(&value, 0))
}
fn service_parent_response(id: u64, result: Result<Reply, HostLifetimeError>) -> Response {
    let reply = response(id, result);
    if private_reply_fits(&reply) { reply } else { response(id, Err(HostLifetimeError::Unavailable)) }
}

fn service_receipt_wire_value(value: &serde_json::Value, containers: usize) -> bool {
    const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
    match value {
        serde_json::Value::Number(number) => {
            if let Some(value) = number.as_u64() {
                value <= MAX_SAFE_INTEGER
            } else if let Some(value) = number.as_i64() {
                value >= -(MAX_SAFE_INTEGER as i64) && value <= MAX_SAFE_INTEGER as i64
            } else {
                // An f64 such as 1e20 is integral to JavaScript too. Checking
                // only serde's integer variants would still break its decoder.
                number.as_f64().is_some_and(|value| value.is_finite()
                    && (value.fract() != 0.0 || value.abs() <= MAX_SAFE_INTEGER as f64))
            }
        }
        serde_json::Value::Array(values) => containers < 128
            && values.iter().all(|value| service_receipt_wire_value(value, containers + 1)),
        serde_json::Value::Object(values) => containers < 128
            && values.values().all(|value| service_receipt_wire_value(value, containers + 1)),
        _ => true,
    }
}

fn response(id: u64, result: Result<Reply, HostLifetimeError>) -> Response {
    Response {
        version: PRIVATE_HOST_PROTOCOL_VERSION,
        id,
        result: result.unwrap_or_else(|error| Reply::Error {
            error: error.into(),
        }),
    }
}

/// Adopt at process entry, before constructing Tokio or spawning any child.
/// Originals are made close-on-exec first, then closed. Safe owned File copies
/// carry the exact local endpoint identity; no raw-fd ownership is manufactured with unsafe.
pub struct InheritedHostPipes {
    commands: File,
    responses: File,
}

struct InheritedDescriptors([Option<RawFd>; 2]);
impl Drop for InheritedDescriptors {
    fn drop(&mut self) {
        for fd in self.0.into_iter().flatten() {
            let _ = nix::unistd::close(fd);
        }
    }
}

impl InheritedHostPipes {
    pub fn adopt(commands: RawFd, responses: RawFd) -> Result<Self, PrivateHostError> {
        // This function owns every distinct non-stdio input from entry, even
        // when another input is malformed. Never close a standard descriptor.
        let originals = InheritedDescriptors([
            (commands >= 3).then_some(commands),
            (responses >= 3 && responses != commands).then_some(responses),
        ]);
        if commands < 3 || responses < 3 || commands == responses {
            return Err(PrivateHostError::Protocol);
        }
        for fd in originals.0.into_iter().flatten() {
            use nix::fcntl::{FcntlArg, FdFlag, fcntl};
            let flags = fcntl(fd, FcntlArg::F_GETFD).map_err(io::Error::from)?;
            fcntl(
                fd,
                FcntlArg::F_SETFD(FdFlag::from_bits_truncate(flags) | FdFlag::FD_CLOEXEC),
            )
            .map_err(io::Error::from)?;
        }
        let commands = duplicate_endpoint(commands, false)?;
        let responses = duplicate_endpoint(responses, true)?;
        drop(originals);
        Ok(Self {
            commands,
            responses,
        })
    }

    fn into_async(
        self,
    ) -> Result<
        (
            Box<dyn AsyncRead + Unpin + Send>,
            Box<dyn AsyncWrite + Unpin + Send>,
        ),
        PrivateHostError,
    > {
        // Tokio's pipe types reject sockets. Use the native stream type after
        // endpoint validation; there is no unchecked pipe conversion.
        let commands: Box<dyn AsyncRead + Unpin + Send> =
            if self.commands.metadata()?.file_type().is_socket() {
                Box::new(into_local_stream(self.commands)?)
            } else {
                Box::new(tokio::net::unix::pipe::Receiver::from_file(self.commands)?)
            };
        let responses: Box<dyn AsyncWrite + Unpin + Send> =
            if self.responses.metadata()?.file_type().is_socket() {
                Box::new(into_local_stream(self.responses)?)
            } else {
                Box::new(tokio::net::unix::pipe::Sender::from_file(self.responses)?)
            };
        Ok((commands, responses))
    }
}

fn into_local_stream(file: File) -> Result<tokio::net::UnixStream, PrivateHostError> {
    let descriptor: OwnedFd = file.into();
    let stream = std::os::unix::net::UnixStream::from(descriptor);
    stream.set_nonblocking(true)?;
    Ok(tokio::net::UnixStream::from_std(stream)?)
}

fn duplicate_endpoint(fd: RawFd, writing: bool) -> Result<File, PrivateHostError> {
    use nix::{
        fcntl::{FcntlArg, OFlag, fcntl},
        sys::{
            socket::{SockType, UnixAddr, getpeername, getsockname, getsockopt, sockopt},
            stat::{SFlag, fstat},
        },
    };
    let stat = fstat(fd).map_err(io::Error::from)?;
    let kind = SFlag::from_bits_truncate(stat.st_mode).intersection(SFlag::S_IFMT);
    let socket = match kind {
        SFlag::S_IFIFO => false,
        SFlag::S_IFSOCK => true,
        _ => return Err(PrivateHostError::Protocol),
    };
    let mode = OFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFL).map_err(io::Error::from)?)
        & OFlag::O_ACCMODE;
    let expected = if socket {
        OFlag::O_RDWR
    } else if writing {
        OFlag::O_WRONLY
    } else {
        OFlag::O_RDONLY
    };
    if mode != expected {
        return Err(PrivateHostError::Protocol);
    }
    let file = if socket {
        // Linux cannot reopen a socket through /dev/fd. Reserve an exclusively
        // owned descriptor, then replace that slot without manufacturing raw
        // ownership. Adoption runs before Tokio or child creation; no other
        // code may close/reuse the supplied descriptors during this transfer.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")?;
        nix::unistd::dup2(fd, file.as_raw_fd()).map_err(io::Error::from)?;
        // dup2 clears CLOEXEC. Restore it before anything may spawn a child,
        // and retain all existing status flags when enabling nonblocking I/O.
        let flags = fcntl(file.as_raw_fd(), FcntlArg::F_GETFD).map_err(io::Error::from)?;
        fcntl(
            file.as_raw_fd(),
            FcntlArg::F_SETFD(
                nix::fcntl::FdFlag::from_bits_truncate(flags) | nix::fcntl::FdFlag::FD_CLOEXEC,
            ),
        )
        .map_err(io::Error::from)?;
        let flags = fcntl(file.as_raw_fd(), FcntlArg::F_GETFL).map_err(io::Error::from)?;
        fcntl(
            file.as_raw_fd(),
            FcntlArg::F_SETFL(OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK),
        )
        .map_err(io::Error::from)?;
        file
    } else {
        OpenOptions::new()
            .read(!writing)
            .write(writing)
            .custom_flags(OFlag::O_NONBLOCK.bits())
            .open(format!("/dev/fd/{fd}"))?
    };
    let metadata = file.metadata()?;
    if metadata.dev() != stat.st_dev as u64 || metadata.ino() != stat.st_ino as u64 {
        return Err(PrivateHostError::Protocol);
    }
    if socket {
        // Validate the owned duplicate itself. UnixAddr conversion verifies
        // AF_UNIX; getpeername refuses unconnected endpoints.
        let _: UnixAddr = getsockname(file.as_raw_fd()).map_err(|_| PrivateHostError::Protocol)?;
        let _: UnixAddr = getpeername(file.as_raw_fd()).map_err(|_| PrivateHostError::Protocol)?;
        if getsockopt(&file, sockopt::SockType).map_err(io::Error::from)? != SockType::Stream
        {
            return Err(PrivateHostError::Protocol);
        }
    }
    let flags = fcntl(file.as_raw_fd(), FcntlArg::F_GETFD).map_err(io::Error::from)?;
    if !nix::fcntl::FdFlag::from_bits_truncate(flags).contains(nix::fcntl::FdFlag::FD_CLOEXEC) {
        return Err(PrivateHostError::Protocol);
    }
    Ok(file)
}

async fn read_request(
    reader: &mut (impl AsyncBufRead + Unpin),
    frame: &mut Vec<u8>,
) -> Result<Option<Request>, PrivateHostError> {
    // `frame` belongs to the retained reader owner. A select branch may cancel
    // this observation after consuming a prefix without losing those bytes.
    if frame.len() as u64 > MAX_FRAME_BYTES {
        return Err(PrivateHostError::Protocol);
    }
    let remaining = MAX_FRAME_BYTES + 1 - frame.len() as u64;
    let count = (&mut *reader)
        .take(remaining)
        .read_until(b'\n', frame)
        .await?;
    if count == 0 && frame.is_empty() {
        return Ok(None);
    }
    if frame.len() as u64 > MAX_FRAME_BYTES || frame.last() != Some(&b'\n') {
        return Err(PrivateHostError::Protocol);
    }
    let request: Request = serde_json::from_slice(frame).map_err(|_| PrivateHostError::Protocol)?;
    frame.clear();
    if request.version != PRIVATE_HOST_PROTOCOL_VERSION {
        return Err(PrivateHostError::Protocol);
    }
    Ok(Some(request))
}

async fn write_response(
    writer: &mut (impl AsyncWrite + Unpin),
    response: &Response,
) -> Result<(), PrivateHostError> {
    let mut frame = serde_json::to_vec(response).map_err(|_| PrivateHostError::Protocol)?;
    frame.push(b'\n');
    if frame.len() as u64 > MAX_FRAME_BYTES {
        return Err(PrivateHostError::Protocol);
    }
    writer.write_all(&frame).await?;
    writer.flush().await?;
    Ok(())
}

/// One retained private connection. Dropping this observation closes admission;
/// its server task retains the real owner through physical close settlement.
#[derive(Clone)]
pub struct PrivateHostClosed(pub(crate) watch::Receiver<bool>);

impl PrivateHostClosed {
    pub(crate) fn is_closed(&self) -> bool {
        *self.0.borrow() || self.0.has_changed().is_err()
    }

    /// Read-only presenter closure observation; it grants no admission.
    pub fn is_closed_for_presenter(&self) -> bool { self.is_closed() }

    pub async fn wait_closed(&self) {
        let mut closed = self.0.clone();
        loop {
            if *closed.borrow_and_update() {
                return;
            }
            if closed.changed().await.is_err() {
                return;
            }
        }
    }
}

/// One request on the existing inherited channel. The command carries only
/// authenticated source submission facts. The local executable retains the
/// native pin and operator view; no operator identity or decision is accepted
/// from the parent. Dropping the response cannot cancel already admitted work.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ParentReviewKind { Controller, SetupRequest }

pub struct LocalControllerReviewRequest {
    pub purpose: ParentReviewKind,
    pub submission: PrivateOriginalSubmit,
    pub response: oneshot::Sender<Result<OperatorParentOutcome, HostLifetimeError>>,
}

/// Single local presenter queue, selected only by executable composition.
/// It is not a grant owner or a remote operator authentication mechanism.
pub struct LocalControllerReviews(mpsc::Receiver<LocalControllerReviewRequest>);
impl LocalControllerReviews {
    pub async fn next(&mut self) -> Option<LocalControllerReviewRequest> {
        self.0.recv().await
    }
}

/// Opaque authenticated parent invocation. It is not a principal or proof of
/// root authority. The parent owns authentication and retires this invocation.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteControllerLogin {
    pub scope_id: String,
    pub expires_at_ms: u64,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteReviewDecision { Approve, Deny }
#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteReviewStatus {
    AwaitingPrompt,
    Prompt { prompt: serde_json::Value },
    /// Forwarded to the native presenter, not yet a grant or accepted decision.
    AnswerForwarded,
    Retired,
}
pub type RemoteReviewReply = oneshot::Sender<Result<RemoteReviewStatus, HostLifetimeError>>;
/// Bounded per-review transport custody. The actual native approval remains in
/// LocalApprovals and the original source/store owner remains in this gateway.
pub enum RemoteControllerCommand {
    ServiceParent(ServiceParentCommand),
    Begin {
        purpose: ParentReviewKind,
        review_id: u64,
        submission: PrivateOriginalSubmit,
        login: RemoteControllerLogin,
        response: oneshot::Sender<Result<OperatorParentOutcome, HostLifetimeError>>,
    },
    Observe { review_id: u64, response: RemoteReviewReply },
    Answer {
        review_id: u64, approval_id: String, decision: RemoteReviewDecision,
        response: RemoteReviewReply,
    },
    Retire { review_id: u64, response: RemoteReviewReply },
}
impl RemoteControllerCommand {
    pub async fn refuse(self) {
        match self {
            Self::ServiceParent(command) => command.refuse().await,
            Self::Begin { response, .. } => { let _ = response.send(Err(HostLifetimeError::Unavailable)); }
            Self::Observe { response, .. } | Self::Answer { response, .. }
            | Self::Retire { response, .. } => { let _ = response.send(Err(HostLifetimeError::Unavailable)); }
        }
    }
}
pub struct RemoteControllerReviews(mpsc::Receiver<RemoteControllerCommand>);
impl RemoteControllerReviews {
    /// Transfer the one receiver to the existing protected presenter loop.
    pub fn into_receiver(self) -> mpsc::Receiver<RemoteControllerCommand> { self.0 }
    pub async fn next(&mut self) -> Option<RemoteControllerCommand> { self.0.recv().await }
}

// Owns the existing pipe-close watch and forwards its mutations through the
// same native publication as the host and original-task issuer. No second
// closed flag, epoch or authentication source is maintained.
#[derive(Clone)]
struct PrivateHostClosePublisher {
    closed: watch::Sender<bool>,
    publication: Option<LocalAuthorizationPublication>,
}

impl PrivateHostClosePublisher {
    fn close(&self) -> Result<(), HostLifetimeError> {
        // Invoke only after host phase mutation returns. Acquiring publication
        // before host_lost/begin_close_activation would invert their lock order.
        let publication = self.publication.as_ref()
            .map(LocalAuthorizationPublication::begin_owner_change)
            .transpose();
        // A poisoned/exhausted publication already refuses every observation.
        // Pipe loss is still a real fact, even when that or host cleanup fails.
        self.closed.send_replace(true);
        publication.map(|_| ()).map_err(|_| HostLifetimeError::Unavailable)
    }
}

pub struct PrivateHostConnection {
    service_parent: Option<ServiceParentEndpoint>,
    console_logins: ConsoleLogins,
    owner: Arc<dyn PrivateHostLifetimeControl>,
    closed: watch::Receiver<bool>,
    close_publisher: PrivateHostClosePublisher,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<Result<(), PrivateHostError>>>,
}

struct HostLossGuard {
    owner: Arc<dyn PrivateHostLifetimeControl>,
    closed: PrivateHostClosePublisher,
    armed: bool,
}
impl Drop for HostLossGuard {
    fn drop(&mut self) {
        if self.armed {
            let close = self.owner.host_lost();
            let _ = self.closed.close();
            if let Ok(observation) = close {
                let owner = self.owner.clone();
                // A dropped waiter does not retire the physical close owner.
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    runtime.spawn(async move {
                        let _ = owner.join_close(&observation.handle).await;
                    });
                }
                // Executor/process loss has no settlement attestation. It
                // cannot be repaired by pretending a task was joined here.
            }
        }
    }
}

impl PrivateHostConnection {
    /// The factory is a native process composition parameter, never selected by
    /// public JSON. It must construct the actual operation-owning lifetime from
    /// these verified selectors. The returned Arc is the same owner the gateway
    /// registers, not a second policy callback or an Allow-response cache.
    pub async fn bootstrap<O: PrivateHostLifetimeControl + 'static>(
        pipes: InheritedHostPipes,
        factory: impl FnOnce(HostBootstrapSelection) -> Result<O, HostLifetimeError>,
    ) -> Result<(Arc<O>, Self), PrivateHostError> {
        let (owner, _, connection) = Self::bootstrap_composed(pipes, |selection| async move {
            factory(selection).map(|owner| (owner, None))
        }, None, None)
        .await?;
        Ok((owner, connection))
    }

    /// Select the concrete native original adapter at process composition.
    /// The same inherited channel carries its commands; public JSON cannot
    /// install this adapter. Source retention is explicitly pending admission.
    pub async fn bootstrap_with_originals<O, F>(
        pipes: InheritedHostPipes,
        factory: impl FnOnce(HostBootstrapSelection) -> F,
    ) -> Result<(Arc<O>, Arc<PrivateOriginals>, Self), PrivateHostError>
    where
        O: PrivateHostLifetimeControl + 'static,
        F: std::future::Future<Output = Result<(O, Arc<PrivateOriginals>), HostLifetimeError>>,
    {
        let (owner, originals, connection) =
            Self::bootstrap_composed(pipes, |selection| async move {
                factory(selection)
                    .await
                    .map(|(owner, originals)| (owner, Some(originals)))
            }, None, None)
            .await?;
        Ok((
            owner,
            originals.ok_or(PrivateHostError::Protocol)?,
            connection,
        ))
    }

    /// The existing gateway executable may consume these requests with its
    /// explicitly owned local terminal. Until a receiver is retained, requests
    /// are unavailable. Public RPC and bootstrap JSON cannot enable an operator.
    pub async fn bootstrap_with_operator_reviews<O, F>(
        pipes: InheritedHostPipes,
        factory: impl FnOnce(HostBootstrapSelection) -> F,
    ) -> Result<(Arc<O>, Arc<PrivateOriginals>, Self, LocalControllerReviews), PrivateHostError>
    where
        O: PrivateHostLifetimeControl + 'static,
        F: std::future::Future<Output = Result<(O, Arc<PrivateOriginals>), HostLifetimeError>>,
    {
        let (send, receive) = mpsc::channel(1);
        let (owner, originals, connection) = Self::bootstrap_composed(
            pipes,
            |selection| async move {
                factory(selection).await.map(|(owner, originals)| (owner, Some(originals)))
            },
            Some(send),
            None,
        ).await?;
        Ok((owner, originals.ok_or(PrivateHostError::Protocol)?, connection,
            LocalControllerReviews(receive)))
    }

    /// Existing executable composition with both protected presenter surfaces.
    /// Remote authority is still selected solely by the installed native policy.
    pub async fn bootstrap_with_controller_reviews<O, F>(
        pipes: InheritedHostPipes,
        factory: impl FnOnce(HostBootstrapSelection) -> F,
    ) -> Result<(Arc<O>, Arc<PrivateOriginals>, Self, LocalControllerReviews, RemoteControllerReviews), PrivateHostError>
    where
        O: PrivateHostLifetimeControl + 'static,
        F: std::future::Future<Output = Result<(O, Arc<PrivateOriginals>), HostLifetimeError>>,
    {
        let (local, local_receive) = mpsc::channel(1);
        let (remote, remote_receive) = mpsc::channel(1);
        let (owner, originals, connection) = Self::bootstrap_composed(pipes, |selection| async move {
            factory(selection).await.map(|(owner, originals)| (owner, Some(originals)))
        }, Some(local), Some(remote)).await?;
        Ok((owner, originals.ok_or(PrivateHostError::Protocol)?, connection,
            LocalControllerReviews(local_receive), RemoteControllerReviews(remote_receive)))
    }

    async fn bootstrap_composed<O, F>(
        pipes: InheritedHostPipes,
        factory: impl FnOnce(HostBootstrapSelection) -> F,
        operator_reviews: Option<mpsc::Sender<LocalControllerReviewRequest>>,
        remote_reviews: Option<mpsc::Sender<RemoteControllerCommand>>,
    ) -> Result<(Arc<O>, Option<Arc<PrivateOriginals>>, Self), PrivateHostError>
    where
        O: PrivateHostLifetimeControl + 'static,
        F: std::future::Future<
                Output = Result<(O, Option<Arc<PrivateOriginals>>), HostLifetimeError>,
            >,
    {
        let startup = PrivateHostStartup::read(pipes).await?;
        Self::bootstrap_started(startup, factory, operator_reviews, remote_reviews).await
    }

    /// Continue the one startup reader after the actual preparation driver has
    /// returned its completed artifact and received the final Bootstrap. This
    /// only adopts the final private owner; native Ready remains native-owned.
    pub async fn bootstrap_from_startup_with_controller_reviews<O, F>(
        startup: PrivateHostStartup,
        factory: impl FnOnce(HostBootstrapSelection) -> F,
    ) -> Result<(Arc<O>, Arc<PrivateOriginals>, Self, LocalControllerReviews, RemoteControllerReviews), PrivateHostError>
    where
        O: PrivateHostLifetimeControl + 'static,
        F: std::future::Future<Output = Result<(O, Arc<PrivateOriginals>), HostLifetimeError>>,
    {
        let (local, local_receive) = mpsc::channel(1);
        let (remote, remote_receive) = mpsc::channel(1);
        let (owner, originals, connection) = Self::bootstrap_started(startup, |selection| async move {
            factory(selection).await.map(|(owner, originals)| (owner, Some(originals)))
        }, Some(local), Some(remote)).await?;
        Ok((owner, originals.ok_or(PrivateHostError::Protocol)?, connection,
            LocalControllerReviews(local_receive), RemoteControllerReviews(remote_receive)))
    }

    async fn bootstrap_started<O, F>(
        startup: PrivateHostStartup,
        factory: impl FnOnce(HostBootstrapSelection) -> F,
        operator_reviews: Option<mpsc::Sender<LocalControllerReviewRequest>>,
        remote_reviews: Option<mpsc::Sender<RemoteControllerCommand>>,
    ) -> Result<(Arc<O>, Option<Arc<PrivateOriginals>>, Self), PrivateHostError>
    where
        O: PrivateHostLifetimeControl + 'static,
        F: std::future::Future<Output = Result<(O, Option<Arc<PrivateOriginals>>), HostLifetimeError>>,
    {
        let (owner, originals, connection, ()) = Self::bootstrap_started_with_finish(
            startup, factory, operator_reviews, remote_reviews,
            |_, _, _, _| async { Ok(()) },
        ).await?;
        Ok((owner, originals, connection))
    }

    /// Complete the same final installed composition before acknowledging
    /// Bootstrap. The closure receives this connection's actual owner, close
    /// observer and login registry; it cannot substitute a second connection.
    pub async fn bootstrap_prepared_with_controller_reviews<O, F, T, C>(
        startup: PrivateHostStartup,
        factory: impl FnOnce(HostBootstrapSelection) -> F,
        finish: impl FnOnce(Arc<O>, PrivateHostClosed, ConsoleLogins) -> C,
    ) -> Result<(T, Self, LocalControllerReviews, RemoteControllerReviews), PrivateHostError>
    where
        O: PrivateHostLifetimeControl + 'static,
        F: std::future::Future<Output = Result<(O, Arc<PrivateOriginals>), HostLifetimeError>>,
        C: std::future::Future<Output = Result<T, HostLifetimeError>>,
    {
        let (local, local_receive) = mpsc::channel(1);
        let (remote, remote_receive) = mpsc::channel(1);
        let (_, originals, connection, installed) = Self::bootstrap_started_with_finish(
            startup, |selection| async move {
                factory(selection).await.map(|(owner, originals)| (owner, Some(originals)))
            }, Some(local), Some(remote), move |owner, closed, logins, _| finish(owner, closed, logins),
        ).await?;
        originals.ok_or(PrivateHostError::Protocol)?;
        Ok((installed, connection, LocalControllerReviews(local_receive), RemoteControllerReviews(remote_receive)))
    }

    /// Same final owner and connection, with its existing remote actor endpoint.
    /// No administrative operation starts before the final composition returns.
    pub async fn bootstrap_prepared_with_service_parent<O, F, T, C>(
        startup: PrivateHostStartup,
        factory: impl FnOnce(HostBootstrapSelection) -> F,
        finish: impl FnOnce(Arc<O>, PrivateHostClosed, ConsoleLogins, ServiceParentEndpoint) -> C,
    ) -> Result<(T, Self, LocalControllerReviews, RemoteControllerReviews), PrivateHostError>
    where O: PrivateHostLifetimeControl + 'static,
        F: std::future::Future<Output = Result<(O, Arc<PrivateOriginals>), HostLifetimeError>>,
        C: std::future::Future<Output = Result<T, HostLifetimeError>>,
    {
        let (local, local_receive) = mpsc::channel(1);
        let (remote, remote_receive) = mpsc::channel(1);
        let (_, originals, connection, installed) = Self::bootstrap_started_with_finish(
            startup, |selection| async move {
                factory(selection).await.map(|(owner, originals)| (owner, Some(originals)))
            }, Some(local), Some(remote), move |owner, closed, logins, endpoint| async move {
                finish(owner, closed, logins, endpoint.ok_or(HostLifetimeError::Unavailable)?).await
            },
        ).await?;
        originals.ok_or(PrivateHostError::Protocol)?;
        Ok((installed, connection, LocalControllerReviews(local_receive), RemoteControllerReviews(remote_receive)))
    }

    async fn bootstrap_started_with_finish<O, F, T, C>(
        startup: PrivateHostStartup,
        factory: impl FnOnce(HostBootstrapSelection) -> F,
        operator_reviews: Option<mpsc::Sender<LocalControllerReviewRequest>>,
        remote_reviews: Option<mpsc::Sender<RemoteControllerCommand>>,
        finish: impl FnOnce(Arc<O>, PrivateHostClosed, ConsoleLogins, Option<ServiceParentEndpoint>) -> C,
    ) -> Result<(Arc<O>, Option<Arc<PrivateOriginals>>, Self, T), PrivateHostError>
    where
        O: PrivateHostLifetimeControl + 'static,
        F: std::future::Future<Output = Result<(O, Option<Arc<PrivateOriginals>>), HostLifetimeError>>,
        C: std::future::Future<Output = Result<T, HostLifetimeError>>,
    {
        let (reader, mut responses, request_id, selection) = startup.into_installed()?;
        let (owner, originals) = match factory(selection).await {
            Ok((owner, originals)) => (Arc::new(owner), originals),
            Err(error) => {
                write_response(&mut responses, &response(request_id, Err(error))).await?;
                return Err(error.into());
            }
        };
        let erased: Arc<dyn PrivateHostLifetimeControl> = owner.clone();
        let (closed_tx, closed) = watch::channel(false);
        let publication = originals.as_ref().map(|source| source.authorization_publication());
        let close_publisher = PrivateHostClosePublisher {
            closed: closed_tx,
            publication: publication.clone(),
        };
        let mut loss = HostLossGuard {
            owner: erased.clone(),
            closed: close_publisher.clone(),
            armed: true,
        };
        let console_logins = ConsoleLogins::new(PrivateHostClosed(closed.clone()), publication);
        let service_parent = remote_reviews.clone().map(|sender| ServiceParentEndpoint {
            sender, owner: erased.clone(), closed: PrivateHostClosed(closed.clone()), logins: console_logins.clone(),
        });
        let installed = finish(owner.clone(), PrivateHostClosed(closed.clone()), console_logins.clone(), service_parent.clone()).await;
        let (installed, acknowledged) = match installed {
            Ok(installed) => (Some(installed), write_response(
                &mut responses, &response(request_id, Ok(Reply::BootstrapAccepted)),
            ).await),
            Err(error) => {
                let response = write_response(&mut responses, &response(request_id, Err(error))).await;
                (None, response.and(Err(error.into())))
            }
        };
        if let Err(error) = acknowledged {
            // Ordinary handshake failure keeps this owner until its actual
            // close settles; the process entry point must not drop the executor
            // while a guard-created cleanup task is still pending.
            let close = erased.host_lost();
            let publication = close_publisher.close();
            let settled = match close {
                Ok(observation) => erased.join_close(&observation.handle).await,
                Err(error) => Err(error),
            };
            loss.armed = false;
            match settled {
                Ok(observation) if observation.state == HostCloseState::Settled => {
                    publication?;
                    return Err(error);
                }
                Ok(_) => return Err(PrivateHostError::NotSettled),
                Err(error) => return Err(error.into()),
            }
        }
        let installed = installed.ok_or(PrivateHostError::Protocol)?;
        let (stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve(
            reader,
            responses,
            erased.clone(),
            originals.clone(),
            operator_reviews,
            remote_reviews,
            console_logins.clone(),
            close_publisher.clone(),
            stop_rx,
            request_id + 1,
        ));
        loss.armed = false;
        Ok((
            owner,
            originals,
            Self {
                service_parent,
                console_logins,
                owner: erased,
                closed,
                close_publisher,
                stop,
                task: Some(task),
            },
            installed,
        ))
    }

    /// Share this exact connection owner with the installed native ingress and presenter.
    /// Enrollment labels alone do not authenticate either caller.
    pub fn service_parent_endpoint(&self) -> Result<ServiceParentEndpoint, HostLifetimeError> {
        self.service_parent.clone().ok_or(HostLifetimeError::Unavailable)
    }

    pub fn console_logins(&self) -> ConsoleLogins { self.console_logins.clone() }

    pub fn closed_observer(&self) -> PrivateHostClosed {
        PrivateHostClosed(self.closed.clone())
    }

    pub async fn wait_closed(&self) {
        self.closed_observer().wait_closed().await;
    }

    pub async fn finish(mut self) -> Result<(), PrivateHostError> {
        self.stop.send_replace(true);
        match self.task.take() {
            Some(task) => match task.await {
                Ok(result) => result,
                Err(_) => {
                    let close = self.owner.host_lost();
                    let publication = self.close_publisher.close();
                    let close = close?;
                    let settled = self.owner.join_close(&close.handle).await?;
                    if settled.state != HostCloseState::Settled {
                        return Err(PrivateHostError::NotSettled);
                    }
                    publication?;
                    Err(PrivateHostError::OwnerTask)
                }
            },
            None => Err(PrivateHostError::OwnerTask),
        }
    }
}
impl Drop for PrivateHostConnection {
    fn drop(&mut self) {
        if self.task.is_some() {
            let _ = self.owner.host_lost();
            let _ = self.close_publisher.close();
            self.stop.send_replace(true);
        }
    }
}

async fn serve<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin + Send + 'static>(
    mut reader: R,
    mut writer: W,
    owner: Arc<dyn PrivateHostLifetimeControl>,
    originals: Option<Arc<PrivateOriginals>>,
    operator_reviews: Option<mpsc::Sender<LocalControllerReviewRequest>>,
    remote_reviews: Option<mpsc::Sender<RemoteControllerCommand>>,
    console_logins: ConsoleLogins,
    closed: PrivateHostClosePublisher,
    mut stop: watch::Receiver<bool>,
    mut next_id: u64,
) -> Result<(), PrivateHostError> {
    let mut loss = HostLossGuard {
        owner: owner.clone(),
        closed: closed.clone(),
        armed: true,
    };
    let (reply_tx, mut reply_rx) = mpsc::channel::<Response>(32);
    let (writer_lost_tx, mut writer_lost) = watch::channel(false);
    let writer_task = tokio::spawn(async move {
        while let Some(response) = reply_rx.recv().await {
            if write_response(&mut writer, &response).await.is_err() {
                writer_lost_tx.send_replace(true);
                return;
            }
        }
    });
    let mut joins = JoinSet::new();
    // Separate bounded waiters keep retirement independent of ordinary work and
    // presenter capacity. The reader publishes its fence before reserving a waiter.
    let mut retirements = JoinSet::new();
    let mut frame = Vec::new();
    let (normal_close, mut failure) = loop {
        let request = tokio::select! {
            _ = stop.changed() => break (true, None),
            _ = writer_lost.changed() => break (false, Some(PrivateHostError::Protocol)),
            joined = retirements.join_next(), if !retirements.is_empty() => {
                match joined {
                    Some(Ok(reply)) => {
                        if reply_tx.try_send(reply).is_err() { break (false, Some(PrivateHostError::Protocol)); }
                    }
                    _ => break (false, Some(PrivateHostError::OwnerTask)),
                }
                continue;
            },
            joined = joins.join_next(), if !joins.is_empty() => {
                match joined {
                    Some(Ok(reply)) => {
                        if reply_tx.try_send(reply).is_err() { break (false, Some(PrivateHostError::Protocol)); }
                    },
                    _ => break (false, Some(PrivateHostError::OwnerTask)),
                }
                continue;
            },
            request = read_request(&mut reader, &mut frame) => match request {
                Ok(Some(request)) => request,
                Ok(None) => break (false, None),
                Err(error) => break (false, Some(error)),
            }
        };
        if request.id != next_id || matches!(&request.command, Command::Bootstrap { .. } | Command::PrepareApplication { .. } | Command::ReplaceApplication { .. }) {
            break (false, Some(PrivateHostError::Protocol));
        }
        let Some(next) = next_id.checked_add(1) else {
            break (false, Some(PrivateHostError::Protocol));
        };
        next_id = next;
        let outcome = match request.command {
            Command::EnrollConsoleLogin { login } => {
                owner.validate_current_selection().and_then(|()| {
                    console_logins.enroll(request.id, login)
                        .map(|()| Reply::ConsoleLoginEnrolled { enrollment_id: request.id })
                })
            }
            Command::RetireConsoleLogin { enrollment_id } => {
                // Do not place this behind native JoinSet or presenter capacity.
                // Even a duplicate/full waiter response occurs after the fence.
                match console_logins.retire(enrollment_id) {
                    Ok(wait) if retirements.len() < 256 => {
                        retirements.spawn(async move { response(request.id, wait.drain().await
                            .map(|observation| Reply::ConsoleLoginRetired { enrollment_id, observation })) });
                        continue;
                    }
                    Ok(_) => Err(HostLifetimeError::NotSettled),
                    Err(error) => Err(error),
                }
            }
            Command::OpenComponent { component } => owner
                .open_component(&component)
                .map(|observation| Reply::Component { observation }),
            Command::BeginCloseComponent { component, reason } => owner
                .begin_close_component(&component, reason)
                .map(|observation| Reply::Close { observation }),
            Command::BeginCloseActivation { reason } => {
                let observation = owner.begin_close_activation(reason);
                observation.and_then(|observation| {
                    closed.close()?;
                    Ok(Reply::Close { observation })
                })
            }
            Command::ObserveClose { handle } => owner
                .observe_close(&handle)
                .map(|observation| Reply::Close { observation }),
            Command::JoinClose { handle } => {
                if joins.len() >= MAX_PENDING_JOINS {
                    Err(HostLifetimeError::Unavailable)
                } else {
                    let owner = owner.clone();
                    joins.spawn(async move {
                        response(
                            request.id,
                            owner
                                .join_close(&handle)
                                .await
                                .map(|observation| Reply::Close { observation }),
                        )
                    });
                    continue;
                }
            }
            command @ (Command::ReviewControllerParent { .. } | Command::ReviewSetupRequest { .. }) => {
                let (purpose, submission) = match command {
                    Command::ReviewControllerParent { submission } => (ParentReviewKind::Controller, submission),
                    Command::ReviewSetupRequest { submission } => (ParentReviewKind::SetupRequest, submission),
                    _ => unreachable!(),
                };
                let selected = if *closed.closed.borrow() {
                    Err(HostLifetimeError::Denied)
                } else {
                    owner.validate_current_selection()
                };
                if let Err(error) = selected {
                    if reply_tx.try_send(response(request.id, Err(error))).is_err() {
                        break (false, Some(PrivateHostError::Protocol));
                    }
                    continue;
                }
                if let Some(presenter) = operator_reviews.as_ref()
                    .filter(|_| joins.len() < MAX_PENDING_JOINS)
                {
                    let (send, receive) = oneshot::channel();
                    if presenter.try_send(LocalControllerReviewRequest {
                        purpose, submission, response: send,
                    }).is_ok() {
                        // The same JoinSet retains this call through parent
                        // loss. The presenter observes closed and retires any
                        // undecided approval, but joins admitted issuance.
                        joins.spawn(async move {
                            response(request.id, receive.await
                                .unwrap_or(Err(HostLifetimeError::Unavailable))
                                .map(|outcome| match purpose {
                                    ParentReviewKind::Controller => Reply::ControllerParentReviewed { outcome },
                                    ParentReviewKind::SetupRequest => Reply::SetupRequestReviewed { outcome },
                                }))
                        });
                        continue;
                    }
                }
                Err(HostLifetimeError::Unavailable)
            }
            command @ (Command::ReviewRemoteControllerParent { .. } | Command::ReviewRemoteSetupRequest { .. }) => {
                let (purpose, submission, login) = match command {
                    Command::ReviewRemoteControllerParent { submission, login } => (ParentReviewKind::Controller, submission, login),
                    Command::ReviewRemoteSetupRequest { submission, login } => (ParentReviewKind::SetupRequest, submission, login),
                    _ => unreachable!(),
                };
                let selected = if *closed.closed.borrow() { Err(HostLifetimeError::Denied) }
                    else { owner.validate_current_selection() };
                if let Err(error) = selected {
                    if reply_tx.try_send(response(request.id, Err(error))).is_err() {
                        break (false, Some(PrivateHostError::Protocol));
                    }
                    continue;
                }
                if let Some(presenter) = remote_reviews.as_ref().filter(|_| joins.len() < MAX_PENDING_JOINS) {
                    let (send, receive) = oneshot::channel();
                    if presenter.try_send(RemoteControllerCommand::Begin {
                        purpose, review_id: request.id, submission, login, response: send,
                    }).is_ok() {
                        joins.spawn(async move { response(request.id, receive.await
                            .unwrap_or(Err(HostLifetimeError::Unavailable))
                            .map(|outcome| match purpose {
                                ParentReviewKind::Controller => Reply::ControllerParentReviewed { outcome },
                                ParentReviewKind::SetupRequest => Reply::SetupRequestReviewed { outcome },
                            })) });
                        continue;
                    }
                }
                Err(HostLifetimeError::Unavailable)
            }
            command @ (Command::ObserveControllerReview { .. }
                | Command::AnswerControllerReview { .. } | Command::RetireControllerReview { .. }) => {
                let selected = if matches!(&command, Command::AnswerControllerReview { .. }) {
                    if *closed.closed.borrow() { Err(HostLifetimeError::Denied) } else { owner.validate_current_selection() }
                } else { Ok(()) };
                if let Err(error) = selected {
                    if reply_tx.try_send(response(request.id, Err(error))).is_err() {
                        break (false, Some(PrivateHostError::Protocol));
                    }
                    continue;
                }
                if let Some(presenter) = remote_reviews.as_ref().filter(|_| joins.len() < MAX_PENDING_JOINS) {
                    let (send, receive) = oneshot::channel();
                    let (review_id, control) = match command {
                        Command::ObserveControllerReview { review_id } => (review_id, RemoteControllerCommand::Observe { review_id, response: send }),
                        Command::AnswerControllerReview { review_id, approval_id, decision } => (review_id, RemoteControllerCommand::Answer { review_id, approval_id, decision, response: send }),
                        Command::RetireControllerReview { review_id } => (review_id, RemoteControllerCommand::Retire { review_id, response: send }),
                        _ => unreachable!(),
                    };
                    if presenter.try_send(control).is_ok() {
                        joins.spawn(async move { response(request.id, receive.await
                            .unwrap_or(Err(HostLifetimeError::Unavailable))
                            .map(|status| Reply::ControllerReview { review_id, status })) });
                        continue;
                    }
                }
                Err(HostLifetimeError::Unavailable)
            }
            command @ (Command::NextServiceParentAdministration
                | Command::SubmitServiceParentAdministration { .. }
                | Command::ResultServiceParentAdministration { .. }
                | Command::RetireServiceParentAdministration { .. }) => {
                let selected = if matches!(&command, Command::SubmitServiceParentAdministration { .. }) {
                    if *closed.closed.borrow() { Err(HostLifetimeError::Denied) } else { owner.validate_current_selection() }
                } else { Ok(()) };
                if let Err(error) = selected {
                    if reply_tx.try_send(response(request.id, Err(error))).is_err() {
                        break (false, Some(PrivateHostError::Protocol));
                    }
                    continue;
                }
                let controlling = matches!(&command, Command::SubmitServiceParentAdministration { .. }
                    | Command::RetireServiceParentAdministration { .. });
                let capacity = if controlling { retirements.len() < 256 } else { joins.len() < MAX_PENDING_JOINS };
                if let Some(sender) = remote_reviews.as_ref().filter(|_| capacity) {
                    // Existing reserved control waiters keep answer/retirement
                    // deliverable when ordinary result observations are full.
                    let service_waiters = if controlling { &mut retirements } else { &mut joins };
                    // Each task is only a response waiter. Actual operation and
                    // answer custody stay on the one remote actor through EOF.
                    match command {
                        Command::NextServiceParentAdministration => {
                            let (send, receive) = oneshot::channel();
                            if sender.try_send(RemoteControllerCommand::ServiceParent(ServiceParentCommand::Next { response: send })).is_ok() {
                                service_waiters.spawn(async move { service_parent_response(request.id, receive.await.unwrap_or(Err(HostLifetimeError::Unavailable))
                                    .map(|request| Reply::ServiceParentAdministrationRequest { exchange_id: request.exchange_id,
                                        proposal_sha256: request.proposal_sha256, proposal: request.proposal })) });
                                continue;
                            }
                        }
                        Command::SubmitServiceParentAdministration { exchange_id, proposal_sha256, decision } => {
                            let (send, receive) = oneshot::channel();
                            if sender.try_send(RemoteControllerCommand::ServiceParent(ServiceParentCommand::Submit { exchange_id, proposal_sha256, decision, response: send })).is_ok() {
                                service_waiters.spawn(async move { service_parent_response(request.id, receive.await.unwrap_or(Err(HostLifetimeError::Unavailable))
                                    .map(|()| Reply::ServiceParentAdministrationSubmitted { exchange_id })) });
                                continue;
                            }
                        }
                        other => {
                            let (send, receive) = oneshot::channel();
                            let (exchange_id, retiring) = match other {
                                Command::ResultServiceParentAdministration { exchange_id } => (exchange_id, false),
                                Command::RetireServiceParentAdministration { exchange_id } => (exchange_id, true),
                                _ => unreachable!(),
                            };
                            let command = if retiring { ServiceParentCommand::Retire { exchange_id, response: send } }
                                else { ServiceParentCommand::Result { exchange_id, response: send } };
                            if sender.try_send(RemoteControllerCommand::ServiceParent(command)).is_ok() {
                                service_waiters.spawn(async move { service_parent_response(request.id, receive.await.unwrap_or(Err(HostLifetimeError::Unavailable))
                                    .map(|observation| if retiring { Reply::ServiceParentAdministrationRetired { exchange_id, observation } }
                                        else { Reply::ServiceParentAdministrationResult { exchange_id, observation } })) });
                                continue;
                            }
                        }
                    }
                }
                Err(HostLifetimeError::Unavailable)
            }
            Command::ObserveServiceInputReceipt { original } => {
                // Historical observation remains available independently of
                // execution admission. The inherited channel and canonical
                // source namespace select the reader; no grant is restored.
                if let Some(originals) = originals.as_ref().filter(|_| joins.len() < MAX_PENDING_JOINS) {
                    let originals = originals.clone();
                    joins.spawn(async move {
                        service_input_response(request.id,
                            originals.observe_service_input_receipt(&original).await
                                .map(|observation| Reply::ServiceInputReceipt { original, observation }))
                    });
                    continue;
                }
                Err(HostLifetimeError::Unavailable)
            }
            Command::SubmitAuthenticatedOriginal { submission } => {
                let selected = if *closed.closed.borrow() {
                    Err(HostLifetimeError::Denied)
                } else {
                    owner.validate_current_selection()
                };
                if let Err(error) = selected {
                    if reply_tx.try_send(response(request.id, Err(error))).is_err() {
                        break (false, Some(PrivateHostError::Protocol));
                    }
                    continue;
                }
                if let Some(originals) = originals
                    .as_ref()
                    .filter(|_| joins.len() < MAX_PENDING_JOINS)
                {
                    let originals = originals.clone();
                    // The existing accepted-command JoinSet survives parent
                    // EOF. Meta additionally owns entered native submission.
                    joins.spawn(async move {
                        let original = submission.original.clone();
                        original_response(
                            request.id,
                            originals.submit_authenticated_original(submission).await.map(
                                |outcome| Reply::OriginalSubmitted { original, outcome },
                            ),
                        )
                    });
                    continue;
                }
                Err(HostLifetimeError::Unavailable)
            }
            Command::RetainAuthenticatedEvent { event } => {
                // The existing irreversible private activation close signal
                // also closes admission of new source mutations. Previously
                // admitted commands remain joined; observe/join stay available.
                let selected = if *closed.closed.borrow() {
                    Err(HostLifetimeError::Denied)
                } else {
                    owner.validate_current_selection()
                };
                if let Err(error) = selected {
                    if reply_tx.try_send(response(request.id, Err(error))).is_err() {
                        break (false, Some(PrivateHostError::Protocol));
                    }
                    continue;
                }
                if let Some(originals) = originals
                    .as_ref()
                    .filter(|_| joins.len() < MAX_PENDING_JOINS)
                {
                    let originals = originals.clone();
                    joins.spawn(async move {
                        original_response(
                            request.id,
                            originals.retain_event(event).await.map(|original| {
                                Reply::AuthenticatedEventRetained {
                                    original,
                                    state: PrivateOriginalRetention::RetainedPendingNativeAdmission,
                                }
                            }),
                        )
                    });
                    continue;
                }
                Err(HostLifetimeError::Unavailable)
            }
            Command::ApplyServiceAccessDecision { decision } => {
                // The same current activation fence as source admission. The
                // joined task owns accepted work through channel shutdown.
                let selected = if *closed.closed.borrow() {
                    Err(HostLifetimeError::Denied)
                } else {
                    owner.validate_current_selection()
                };
                if let Err(error) = selected {
                    if reply_tx.try_send(response(request.id, Err(error))).is_err() {
                        break (false, Some(PrivateHostError::Protocol));
                    }
                    continue;
                }
                if let Some(originals) = originals
                    .as_ref()
                    .filter(|_| joins.len() < MAX_PENDING_JOINS)
                {
                    let originals = originals.clone();
                    joins.spawn(async move {
                        original_response(
                            request.id,
                            originals.apply_service_access_decision(decision).await.map(
                                |observation| Reply::ServiceAccessDecisionApplied { observation },
                            ),
                        )
                    });
                    continue;
                }
                Err(HostLifetimeError::Unavailable)
            }
            Command::CancelOriginalOperation { original } => {
                let selected = if *closed.closed.borrow() {
                    Err(HostLifetimeError::Denied)
                } else {
                    owner.validate_current_selection()
                };
                if let Err(error) = selected {
                    if reply_tx.try_send(response(request.id, Err(error))).is_err() {
                        break (false, Some(PrivateHostError::Protocol));
                    }
                    continue;
                }
                if let Some(originals) = originals
                    .as_ref()
                    .filter(|_| joins.len() < MAX_PENDING_JOINS)
                {
                    let originals = originals.clone();
                    // The existing JoinSet owns admitted persistence and its
                    // readback even after the parent stops reading or exits.
                    joins.spawn(async move {
                        original_response(
                            request.id,
                            originals
                                .cancel_original_operation(&original)
                                .await
                                .map(|state| Reply::OriginalOperationCancelled { original, state }),
                        )
                    });
                    continue;
                }
                Err(HostLifetimeError::Unavailable)
            }
            Command::Bootstrap { .. } | Command::PrepareApplication { .. } | Command::ReplaceApplication { .. } => break (false, Some(PrivateHostError::Protocol)),
        };
        // Backpressure cannot prevent EOF observation or admission closure.
        if reply_tx.try_send(response(request.id, outcome)).is_err() {
            break (false, Some(PrivateHostError::Protocol));
        }
    };
    let close = if normal_close {
        owner.begin_close_activation(HostCloseReason::Shutdown)
    } else {
        owner.host_lost()
    };
    if let Err(error) = closed.close() {
        failure = Some(error.into());
    }
    // Pipe writes only publish observations. Native effect settlement is owned
    // separately; a blocked/unread pipe must not hold its close sequence hostage.
    if !normal_close {
        writer_task.abort();
    }
    let settled = match close {
        Ok(observation) => owner.join_close(&observation.handle).await,
        Err(error) => Err(error),
    };
    while let Some(joined) = joins.join_next().await {
        match joined {
            Ok(reply) if normal_close => {
                let _ = reply_tx.try_send(reply);
            }
            Ok(_) => {}
            Err(_) => failure = Some(PrivateHostError::OwnerTask),
        }
    }
    while let Some(joined) = retirements.join_next().await {
        match joined {
            Ok(reply) if normal_close => { let _ = reply_tx.try_send(reply); }
            Ok(_) => {}
            Err(_) => failure = Some(PrivateHostError::OwnerTask),
        }
    }
    drop(reply_tx);
    let mut writer_task = writer_task;
    if tokio::time::timeout(std::time::Duration::from_secs(1), &mut writer_task)
        .await
        .is_err()
    {
        writer_task.abort();
        let _ = writer_task.await;
    }
    loss.armed = false;
    match settled {
        Ok(observation) if observation.state == HostCloseState::Settled => {}
        Ok(_) => return Err(PrivateHostError::NotSettled),
        Err(error) => return Err(error.into()),
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    mod service_parent_controls;
    use super::*;
    use meerkat_toolkit::host_lifetime::control::{HostCloseScope, HostComponentState};
    use std::{
        os::fd::IntoRawFd,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use tokio::sync::Notify;
    use uuid::Uuid;

    fn numeric_service_receipt(args: serde_json::Value) -> Reply {
        use crate::private_originals::{PrivateServiceInputReceipt, PrivateServiceInputReport,
            PrivateReceiptWitness, PrivateReceiptScope};
        // Serialization/framing control only, not a native finalized receipt.
        Reply::ServiceInputReceipt {
            original: controller_review_submission().original,
            observation: PrivateServiceInputObservation::Admitted {
                session_id: "session".into(), input_id: "input".into(),
                receipt: Some(PrivateServiceInputReceipt {
                    source: PrivateReceiptWitness::LiveRuntime,
                    report: PrivateServiceInputReport::Finalized {
                        input_id: "input".into(), terminal: meerkat_runtime::input_state::InputTerminalOutcome::Consumed,
                        attempt_count: 1, scope: PrivateReceiptScope::Run { run_id: "run".into() }, run_id: Some("run".into()),
                        owner_input_id: "input".into(), recipient_input_ids: vec!["input".into()],
                        outcome: meerkat_runtime::completion::CompletionOutcome::CallbackPending {
                            tool_use_id: "call".into(), tool_name: "read".into(),
                            args,
                        },
                    },
                }),
            },
        }
    }

    #[tokio::test]
    async fn unsupported_service_receipt_is_local_and_next_numeric_reply_survives() {
        let supported = serde_json::json!({"offset": -0.5, "values": [1.25, -7, 1.0],
            "limits": [-9_007_199_254_740_991i64, 9_007_199_254_740_991u64], "negative_zero": -0.0});
        let next = service_input_response(3, Ok(numeric_service_receipt(supported.clone())));
        let expected = serde_json::to_vec(&original_response(3,
            Ok(numeric_service_receipt(supported)))).unwrap();
        // Supported arbitrary numeric data is emitted unchanged, never rounded
        // or normalized to private protocol counter syntax.
        assert_eq!(serde_json::to_vec(&next).unwrap(), expected);
        // The complete serialized reply, including its newline, may fill the
        // shared limit. One extra payload byte must remain a local refusal.
        let empty = original_response(2, Ok(numeric_service_receipt(serde_json::json!({"value": ""}))));
        let payload_limit = MAX_FRAME_BYTES as usize - 1 - serde_json::to_vec(&empty).unwrap().len();
        let at_limit = service_input_response(2, Ok(numeric_service_receipt(
            serde_json::json!({"value": "x".repeat(payload_limit)}))));
        assert!(matches!(&at_limit.result, Reply::ServiceInputReceipt { .. }));
        let mut exact_frame = Vec::new();
        write_response(&mut exact_frame, &at_limit).await.unwrap();
        assert_eq!(exact_frame.len(), MAX_FRAME_BYTES as usize);
        assert_eq!(exact_frame.last(), Some(&b'\n'));
        for unsupported in [
            serde_json::json!({"value": "x".repeat(payload_limit + 1)}),
            serde_json::json!({"value": "x".repeat(MAX_FRAME_BYTES as usize)}),
            serde_json::json!({"value": 9007199254740992u64}),
            serde_json::json!({"nested": [{"value": -9_007_199_254_740_992i64}]}),
            serde_json::json!({"value": u64::MAX}),
            serde_json::json!({"value": 9_007_199_254_740_992.0f64}),
            serde_json::json!({"value": -1e20f64}),
        ] {
            let bounded = service_input_response(2, Ok(numeric_service_receipt(unsupported)));
            assert!(matches!(&bounded.result, Reply::Error { error: WireError::Unavailable }));
            let mut writer = Vec::new();
            write_response(&mut writer, &bounded).await.unwrap();
            write_response(&mut writer, &next).await.unwrap();
            let lines: Vec<_> = writer.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()).collect();
            assert_eq!(lines.len(), 2);
            assert!(lines[0].len() + 1 <= MAX_FRAME_BYTES as usize);
            assert_eq!(lines[1], expected);
            assert_eq!(serde_json::from_slice::<serde_json::Value>(lines[0]).unwrap()["result"]["error"], "unavailable");
        }
    }

    #[tokio::test]
    async fn service_receipt_depth_counts_the_full_envelope_and_preserves_the_next_reply() {
        for depth in [128, 129] {
            // Response/result/observation/receipt/report/outcome are six
            // containers. Scalar args add none; each array below adds one.
            let mut args = serde_json::Value::Null;
            for _ in 6..depth { args = serde_json::Value::Array(vec![args]); }
            let reply = service_input_response(2, Ok(numeric_service_receipt(args)));
            assert_eq!(matches!(&reply.result, Reply::ServiceInputReceipt { .. }), depth == 128);
            if depth == 129 {
                assert!(matches!(&reply.result, Reply::Error { error: WireError::Unavailable }));
            }
            let next = service_input_response(3, Ok(numeric_service_receipt(serde_json::json!({"value": 1.25}))));
            let mut writer = Vec::new();
            write_response(&mut writer, &reply).await.unwrap();
            write_response(&mut writer, &next).await.unwrap();
            let lines: Vec<_> = writer.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()).collect();
            assert_eq!(lines.len(), 2);
            assert!(lines[0].len() + 1 <= MAX_FRAME_BYTES as usize);
            assert_eq!(lines[1], serde_json::to_vec(&next).unwrap());
        }
    }

    #[test]
    fn service_receipt_command_accepts_only_the_original_reference() {
        let original = controller_review_submission().original;
        let command = serde_json::json!({"kind": "observe_service_input_receipt", "original": original});
        assert!(serde_json::from_value::<Command>(command.clone()).is_ok());
        for name in ["session_id", "input_id", "grant", "completed"] {
            let mut changed = command.clone();
            changed[name] = serde_json::json!("caller-selected");
            assert!(serde_json::from_value::<Command>(changed).is_err());
        }
    }

    // Transport-only probe. Native policy/physical custody is tested against
    // NativeHostLifetime in its owner module, not inferred from this probe.
    #[derive(Clone)]
    struct ProbeOwner {
        incarnation: Uuid,
        close: HostCloseHandle,
        lost: Arc<AtomicBool>,
        closed: Arc<AtomicBool>,
        settled: Arc<AtomicBool>,
        joining: Arc<AtomicUsize>,
        wake: Arc<Notify>,
        fail_host_loss: bool,
    }
    impl ProbeOwner {
        fn new(settled: bool) -> Self {
            let incarnation = Uuid::new_v4();
            Self {
                incarnation,
                close: HostCloseHandle {
                    incarnation,
                    close_id: Uuid::new_v4(),
                    scope: HostCloseScope::Activation,
                },
                lost: Arc::new(AtomicBool::new(false)),
                closed: Arc::new(AtomicBool::new(false)),
                settled: Arc::new(AtomicBool::new(settled)),
                joining: Arc::new(AtomicUsize::new(0)),
                wake: Arc::new(Notify::new()),
                fail_host_loss: false,
            }
        }
        fn observation(&self) -> HostCloseObservation {
            HostCloseObservation {
                handle: self.close.clone(),
                state: if self.settled.load(Ordering::Acquire) {
                    HostCloseState::Settled
                } else {
                    HostCloseState::Closing
                },
                failure: None,
            }
        }
        fn settle(&self) {
            self.settled.store(true, Ordering::Release);
            self.wake.notify_waiters();
        }
    }
    #[async_trait::async_trait]
    impl PrivateHostLifetimeControl for ProbeOwner {
        fn validate_current_selection(&self) -> Result<(), HostLifetimeError> {
            if self.closed.load(Ordering::Acquire) {
                Err(HostLifetimeError::Denied)
            } else {
                Ok(())
            }
        }

        fn open_component(
            &self,
            component: &Id,
        ) -> Result<HostComponentObservation, HostLifetimeError> {
            if self.closed.load(Ordering::Acquire) {
                return Err(HostLifetimeError::Denied);
            }
            Ok(HostComponentObservation {
                incarnation: self.incarnation,
                component: component.clone(),
                revision: 1,
                state: HostComponentState::Open,
            })
        }
        fn begin_close_component(
            &self,
            _: &Id,
            _: HostCloseReason,
        ) -> Result<HostCloseObservation, HostLifetimeError> {
            self.begin_close_activation(HostCloseReason::Shutdown)
        }
        fn begin_close_activation(
            &self,
            _: HostCloseReason,
        ) -> Result<HostCloseObservation, HostLifetimeError> {
            self.closed.store(true, Ordering::Release);
            Ok(self.observation())
        }
        fn observe_close(
            &self,
            handle: &HostCloseHandle,
        ) -> Result<HostCloseObservation, HostLifetimeError> {
            if handle != &self.close {
                return Err(HostLifetimeError::WrongOwner);
            }
            Ok(self.observation())
        }
        async fn join_close(
            &self,
            handle: &HostCloseHandle,
        ) -> Result<HostCloseObservation, HostLifetimeError> {
            self.observe_close(handle)?;
            self.joining.fetch_add(1, Ordering::AcqRel);
            loop {
                let wake = self.wake.notified();
                tokio::pin!(wake);
                wake.as_mut().enable();
                if self.settled.load(Ordering::Acquire) {
                    return self.observe_close(handle);
                }
                wake.await;
            }
        }
        fn host_lost(&self) -> Result<HostCloseObservation, HostLifetimeError> {
            self.lost.store(true, Ordering::Release);
            if self.fail_host_loss { return Err(HostLifetimeError::Unavailable); }
            self.begin_close_activation(HostCloseReason::HostLost)
        }
    }
    #[test]
    fn actual_loss_guard_publishes_pipe_exit_even_when_lifetime_refuses_cleanup() {
        let mut probe = ProbeOwner::new(false);
        probe.fail_host_loss = true;
        let owner = Arc::new(probe);
        let publication = LocalAuthorizationPublication::new();
        let (send, receive) = watch::channel(false);
        let closed = PrivateHostClosed(receive);
        let (_, prior) = publication.observe(|| !closed.is_closed()).unwrap();
        drop(HostLossGuard {
            owner: owner.clone(),
            closed: PrivateHostClosePublisher { closed: send, publication: Some(publication) },
            armed: true,
        });
        assert!(owner.lost.load(Ordering::Acquire));
        assert!(closed.is_closed());
        assert!(prior.check_current().is_err());
        assert!(!owner.closed.load(Ordering::Acquire));
        assert_eq!(owner.joining.load(Ordering::Acquire), 0,
            "pipe loss does not manufacture a successful close handle or settlement");
    }

    #[test]
    fn pipe_exit_stays_visible_when_native_publication_is_poisoned() {
        let publication = LocalAuthorizationPublication::new();
        let (_, prior) = publication.observe(|| ()).unwrap();
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _change = publication.begin_owner_change().unwrap();
            panic!("controlled owner mutation failure");
        }));
        assert!(failure.is_err());
        let (send, receive) = watch::channel(false);
        let closed = PrivateHostClosed(receive);
        let publisher = PrivateHostClosePublisher { closed: send, publication: Some(publication) };
        assert_eq!(publisher.close(), Err(HostLifetimeError::Unavailable));
        assert!(closed.is_closed());
        assert!(prior.check_current().is_err());
    }

    fn pipes() -> (InheritedHostPipes, File, File) {
        let (child_read, parent_write) = nix::unistd::pipe().unwrap();
        let (parent_read, child_write) = nix::unistd::pipe().unwrap();
        for fd in [&child_read, &parent_write, &parent_read, &child_write] {
            nix::fcntl::fcntl(
                fd.as_raw_fd(),
                nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC),
            )
            .unwrap();
        }
        let inherited =
            InheritedHostPipes::adopt(child_read.into_raw_fd(), child_write.into_raw_fd()).unwrap();
        (inherited, File::from(parent_write), File::from(parent_read))
    }
    fn selection() -> HostBootstrapSelection {
        use meerkat_toolkit::host_lifetime::control::SelectedHostArtifact;
        let artifact = SelectedHostArtifact {
            path: "/fixture/selected.json".into(),
            sha256: "0".repeat(64),
        };
        HostBootstrapSelection {
            deployment_root: "/fixture".into(),
            activation: artifact.clone(),
            policy_manifest: artifact.clone(),
            local_policy: artifact,
        }
    }
    async fn send(writer: &mut (impl AsyncWrite + Unpin), id: u64, command: Command) {
        let mut bytes = serde_json::to_vec(&Request {
            version: PRIVATE_HOST_PROTOCOL_VERSION,
            id,
            command,
        })
        .unwrap();
        bytes.push(b'\n');
        writer.write_all(&bytes).await.unwrap();
    }

    #[test]
    fn inherited_endpoints_do_not_reach_actual_child_process() {
        let (pipes, _parent_write, _parent_read) = pipes();
        let identities = [
            pipes.commands.metadata().unwrap(),
            pipes.responses.metadata().unwrap(),
        ]
        .map(|metadata| format!("{}:{}", metadata.dev(), metadata.ino()));
        let output = std::process::Command::new("python3")
            .args([
                "-c",
                r#"
import os, sys
for name in os.listdir('/dev/fd'):
    try:
        stat = os.fstat(int(name))
    except OSError:
        continue
    if f'{stat.st_dev}:{stat.st_ino}' in sys.argv[1:]:
        raise SystemExit('private FIFO leaked to child')
"#,
                &identities[0],
                &identities[1],
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn regular_file_and_stdio_are_not_private_pipe_capabilities() {
        assert!(InheritedHostPipes::adopt(0, 1).is_err());
        let first = tempfile::tempfile().unwrap();
        let second = tempfile::tempfile().unwrap();
        assert!(InheritedHostPipes::adopt(first.into_raw_fd(), second.into_raw_fd()).is_err());
    }

    #[test]
    fn malformed_descriptor_pair_closes_every_nonstandard_original_once() {
        use nix::fcntl::{FcntlArg, fcntl};
        let file = tempfile::tempfile().unwrap();
        let fd = file.into_raw_fd();
        assert!(InheritedHostPipes::adopt(0, fd).is_err());
        assert!(fcntl(fd, FcntlArg::F_GETFD).is_err());
        let file = tempfile::tempfile().unwrap();
        let fd = file.into_raw_fd();
        assert!(InheritedHostPipes::adopt(fd, fd).is_err());
        assert!(fcntl(fd, FcntlArg::F_GETFD).is_err());
    }

    #[test]
    fn inherited_sockets_must_be_connected_local_streams() {
        use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};
        let (datagram, _peer) = UnixDatagram::pair().unwrap();
        // Keep the companion endpoint valid so each refusal depends on the
        // first socket's family/type/connected-listener checks.
        let (datagram_response, _datagram_response_peer) = UnixStream::pair().unwrap();
        assert!(
            InheritedHostPipes::adopt(datagram.into_raw_fd(), datagram_response.into_raw_fd())
                .is_err()
        );
        let dir = tempfile::tempdir().unwrap();
        let listener = UnixListener::bind(dir.path().join("not-a-connected-stream")).unwrap();
        let (listener_response, _listener_response_peer) = UnixStream::pair().unwrap();
        assert!(
            InheritedHostPipes::adopt(listener.into_raw_fd(), listener_response.into_raw_fd())
                .is_err()
        );
        let internet = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let stream = std::net::TcpStream::connect(internet.local_addr().unwrap()).unwrap();
        let (_peer, _) = internet.accept().unwrap();
        let (internet_response, _internet_response_peer) = UnixStream::pair().unwrap();
        assert!(
            InheritedHostPipes::adopt(stream.into_raw_fd(), internet_response.into_raw_fd())
                .is_err()
        );
    }

    #[tokio::test]
    async fn inherited_local_streams_use_real_framing_and_parent_eof() {
        use std::os::unix::net::UnixStream;
        let (child_read, parent_write) = UnixStream::pair().unwrap();
        let (parent_read, child_write) = UnixStream::pair().unwrap();
        let pipes =
            InheritedHostPipes::adopt(child_read.into_raw_fd(), child_write.into_raw_fd()).unwrap();
        for endpoint in [&pipes.commands, &pipes.responses] {
            use nix::fcntl::{FcntlArg, FdFlag, OFlag, fcntl};
            let descriptor = fcntl(endpoint.as_raw_fd(), FcntlArg::F_GETFD).unwrap();
            let status = fcntl(endpoint.as_raw_fd(), FcntlArg::F_GETFL).unwrap();
            assert!(FdFlag::from_bits_truncate(descriptor).contains(FdFlag::FD_CLOEXEC));
            assert!(OFlag::from_bits_truncate(status).contains(OFlag::O_NONBLOCK));
        }
        parent_write.set_nonblocking(true).unwrap();
        parent_read.set_nonblocking(true).unwrap();
        let mut commands = tokio::net::UnixStream::from_std(parent_write).unwrap();
        let mut responses = BufReader::new(tokio::net::UnixStream::from_std(parent_read).unwrap());
        send(
            &mut commands,
            1,
            Command::Bootstrap {
                selection: selection(),
            },
        )
        .await;
        let (owner, connection) =
            PrivateHostConnection::bootstrap(pipes, |_| Ok(ProbeOwner::new(true)))
                .await
                .unwrap();
        let mut line = String::new();
        responses.read_line(&mut line).await.unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["result"]["kind"], "bootstrap_accepted");
        drop(commands);
        tokio::time::timeout(std::time::Duration::from_secs(2), connection.wait_closed())
            .await
            .unwrap();
        assert!(owner.lost.load(Ordering::Acquire));
        connection.finish().await.unwrap();
    }

    fn native_selection(root: &std::path::Path) -> HostBootstrapSelection {
        use meerkat_toolkit::host_lifetime::{
            control::SelectedHostArtifact, policy::CompiledLocalPolicy,
        };
        use sha2::{Digest, Sha256};
        let root = root.canonicalize().unwrap();
        let bundle = root.join("shared/policy/bundle.1");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::create_dir_all(root.join("shared/activations")).unwrap();
        std::fs::create_dir_all(root.join("shared/control")).unwrap();
        let artifact = |path: std::path::PathBuf, bytes: Vec<u8>| {
            std::fs::write(&path, &bytes).unwrap();
            SelectedHostArtifact {
                path,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
            }
        };
        let source = artifact(bundle.join("policy.json"), br#"{"schema":1}"#.to_vec());
        // This tests the real selected activation owner, without pretending
        // empty component/source policy grants an operation or authenticates input.
        let local = CompiledLocalPolicy {
            schema: 7,
            native_host: None,
            deployment: Id::new("test").unwrap(),
            source_policy_sha256: source.sha256.clone(),
            components: vec![],
            channels: vec![],
        };
        let local_policy = artifact(
            bundle.join("toolkit-local-policy.json"),
            toolkit_package::canonical_json(&local).unwrap(),
        );
        let policy_manifest = artifact(
            bundle.join("manifest.json"),
            toolkit_package::canonical_json(&serde_json::json!({"artifacts": [
                {"path":"policy.json","sha256":source.sha256},
                {"path":"toolkit-local-policy.json","sha256":local_policy.sha256}
            ]}))
            .unwrap(),
        );
        let activation = artifact(root.join("shared/activations/1.json"), toolkit_package::canonical_json(&serde_json::json!({
            "activation_id":"1", "policy_bundle":{"path":"shared/policy/bundle.1", "policy_digest":format!("hc-policy-v1:sha256:{}",policy_manifest.sha256)}
        })).unwrap());
        std::os::unix::fs::symlink("shared/activations/1.json", root.join("current")).unwrap();
        HostBootstrapSelection {
            deployment_root: root,
            activation,
            policy_manifest,
            local_policy,
        }
    }

    #[tokio::test]
    async fn actual_owner_close_or_selection_replacement_fences_retention() {
        use crate::original_ingress::tests::{initialized, private_event, private_originals};
        use meerkat_toolkit::host_lifetime::NativeHostLifetime;
        use std::os::unix::fs::PermissionsExt;
        for replace_selection in [false, true] {
            let dir = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            let selected = native_selection(dir.path());
            let path = dir.path().join("originals.sqlite3");
            let originals = private_originals(initialized(&dir.path().join("grants")), &path).await;
            let (pipes, command_file, response_file) = pipes();
            let mut commands = tokio::net::unix::pipe::Sender::from_file(command_file).unwrap();
            let mut responses =
                BufReader::new(tokio::net::unix::pipe::Receiver::from_file(response_file).unwrap());
            send(
                &mut commands,
                1,
                Command::Bootstrap {
                    selection: selected.clone(),
                },
            )
            .await;
            let (owner, _, connection) =
                PrivateHostConnection::bootstrap_with_originals(
                    pipes,
                    move |selected| async move {
                        Ok((NativeHostLifetime::bootstrap(selected)?, originals))
                    },
                )
                .await
                .unwrap();
            let mut line = String::new();
            responses.read_line(&mut line).await.unwrap();
            send(
                &mut commands,
                2,
                Command::RetainAuthenticatedEvent {
                    event: private_event("before"),
                },
            )
            .await;
            line.clear();
            responses.read_line(&mut line).await.unwrap();
            let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(reply["result"]["kind"], "authenticated_event_retained");
            let original: PrivateOriginalRef =
                serde_json::from_value(reply["result"]["original"].clone()).unwrap();
            if replace_selection {
                let second = selected.deployment_root.join("shared/activations/2.json");
                std::fs::copy(&selected.activation.path, &second).unwrap();
                std::fs::remove_file(selected.deployment_root.join("current")).unwrap();
                std::os::unix::fs::symlink(
                    "shared/activations/2.json",
                    selected.deployment_root.join("current"),
                )
                .unwrap();
            } else {
                owner
                    .begin_close_activation(HostCloseReason::OperatorDisable)
                    .unwrap();
            }
            // No private close command was sent. The transport's cached signal
            // is still open, so only the actual owner observation can refuse.
            assert!(!*connection.closed_observer().0.borrow());
            send(
                &mut commands,
                3,
                Command::RetainAuthenticatedEvent {
                    event: private_event("after"),
                },
            )
            .await;
            line.clear();
            responses.read_line(&mut line).await.unwrap();
            let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(reply["result"]["kind"], "error");
            assert_eq!(reply["result"]["error"], "denied");
            // Access decisions use the same actual selection fence. A
            // closed host must not write even a pending decision or issue.
            let grant_before = std::fs::read(dir.path().join("grants")).unwrap();
            send(
                &mut commands,
                4,
                Command::ApplyServiceAccessDecision {
                    decision: crate::private_originals::service_grants::tests::decision(),
                },
            )
            .await;
            line.clear();
            responses.read_line(&mut line).await.unwrap();
            let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(reply["result"]["kind"], "error");
            assert_eq!(reply["result"]["error"], "denied");
            assert_eq!(
                std::fs::read(dir.path().join("grants")).unwrap(),
                grant_before
            );
            send(
                &mut commands,
                5,
                Command::CancelOriginalOperation { original },
            )
            .await;
            line.clear();
            responses.read_line(&mut line).await.unwrap();
            let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(reply["result"]["kind"], "error");
            assert_eq!(reply["result"]["error"], "denied");
            let closing = owner
                .begin_close_activation(HostCloseReason::Shutdown)
                .unwrap();
            send(
                &mut commands,
                6,
                Command::ObserveClose {
                    handle: closing.handle,
                },
            )
            .await;
            line.clear();
            responses.read_line(&mut line).await.unwrap();
            let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(reply["result"]["kind"], "close");
            let db = rusqlite::Connection::open(&path).unwrap();
            let count: u64 = db
                .query_row("SELECT count(*) FROM toolkit_records", [], |row| row.get(0))
                .unwrap();
            assert_eq!(
                count, 1,
                "closed/replaced activation admitted another source row"
            );
            let revision: u64 = db
                .query_row("SELECT revision FROM toolkit_records", [], |row| row.get(0))
                .unwrap();
            assert_eq!(revision, 1, "closed/replaced host mutated cancellation");
            connection.finish().await.unwrap();
        }
    }

    #[tokio::test]
    async fn activation_close_refuses_new_source_retention_but_preserves_observation() {
        use crate::original_ingress::tests::{initialized, private_event, private_originals};
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = dir.path().join("originals.sqlite3");
        let originals = private_originals(initialized(&dir.path().join("grants")), &path).await;
        let (pipes, command_file, response_file) = pipes();
        let mut commands = tokio::net::unix::pipe::Sender::from_file(command_file).unwrap();
        let mut responses =
            BufReader::new(tokio::net::unix::pipe::Receiver::from_file(response_file).unwrap());
        send(
            &mut commands,
            1,
            Command::Bootstrap {
                selection: selection(),
            },
        )
        .await;
        let selected = originals.clone();
        let (owner, _, connection) =
            PrivateHostConnection::bootstrap_with_originals(pipes, move |_| async move {
                Ok((ProbeOwner::new(true), selected))
            })
            .await
            .unwrap();
        let mut line = String::new();
        responses.read_line(&mut line).await.unwrap();
        send(
            &mut commands,
            2,
            Command::RetainAuthenticatedEvent {
                event: private_event("before"),
            },
        )
        .await;
        line.clear();
        responses.read_line(&mut line).await.unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["result"]["kind"], "authenticated_event_retained");
        send(
            &mut commands,
            3,
            Command::BeginCloseActivation {
                reason: HostCloseReason::Shutdown,
            },
        )
        .await;
        line.clear();
        responses.read_line(&mut line).await.unwrap();
        assert!(*connection.closed_observer().0.borrow());
        send(
            &mut commands,
            4,
            Command::RetainAuthenticatedEvent {
                event: private_event("after"),
            },
        )
        .await;
        line.clear();
        responses.read_line(&mut line).await.unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["result"]["kind"], "error");
        assert_eq!(reply["result"]["error"], "denied");
        send(
            &mut commands,
            5,
            Command::ObserveClose {
                handle: owner.close.clone(),
            },
        )
        .await;
        line.clear();
        responses.read_line(&mut line).await.unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["result"]["kind"], "close");
        assert_eq!(reply["result"]["observation"]["state"], "settled");
        let connection_db = rusqlite::Connection::open(path).unwrap();
        let count: u64 = connection_db
            .query_row("SELECT count(*) FROM toolkit_records", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "closed activation admitted another source row");
        connection.finish().await.unwrap();
    }

    #[tokio::test]
    async fn parent_loss_keeps_admitted_cancellation_until_exact_store_settlement() {
        use crate::original_ingress::tests::{
            cancellation_store_faults::CancelStore, initialized, principal, private_event,
            private_originals_with_store,
        };
        use meerkat_authorization_contracts::{
            evidence::EvidenceId,
            work_association::{OriginalWorkRef, QualifiedIngressNamespace},
        };
        use meerkat_runtime::meerkat_machine::{AuthenticatedOriginalSource, OriginalDisposition};
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let source = initialized(&dir.path().join("grants"));
        let store = CancelStore::new(&dir.path().join("originals"));
        let originals = private_originals_with_store(source.clone(), store.clone()).await;
        let original = originals
            .retain_event(private_event("parent-loss"))
            .await
            .unwrap();
        let namespace = QualifiedIngressNamespace {
            realm: meerkat_core::RealmId::parse("test-realm").unwrap(),
            ingress: source.ingress_domain().clone(),
            occurrence_scope: EvidenceId::new("parent-loss").unwrap(),
        };
        let work = OriginalWorkRef {
            authority: principal("source"),
            work: original.original.clone(),
        };
        let (pipes, command_file, response_file) = pipes();
        let mut commands = tokio::net::unix::pipe::Sender::from_file(command_file).unwrap();
        let mut responses =
            BufReader::new(tokio::net::unix::pipe::Receiver::from_file(response_file).unwrap());
        send(
            &mut commands,
            1,
            Command::Bootstrap {
                selection: selection(),
            },
        )
        .await;
        let selected = originals.clone();
        let (_, _, connection) =
            PrivateHostConnection::bootstrap_with_originals(pipes, move |_| async move {
                Ok((ProbeOwner::new(true), selected))
            })
            .await
            .unwrap();
        let mut line = String::new();
        responses.read_line(&mut line).await.unwrap();
        store.hold_next_cancellation();
        send(
            &mut commands,
            2,
            Command::CancelOriginalOperation { original },
        )
        .await;
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.wait_for_held_cancellation(),
        )
        .await
        .unwrap();
        assert!(!store.stored_cancelled().await);
        assert!(matches!(
            source.original_disposition(&namespace, &work),
            Err(meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable),
        ));
        drop(commands);
        drop(responses);
        tokio::time::timeout(std::time::Duration::from_secs(2), connection.wait_closed())
            .await
            .unwrap();
        let mut finishing = tokio::spawn(connection.finish());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut finishing)
                .await
                .is_err(),
            "private owner dropped its held cancellation task"
        );
        store.release_and_join_cancellation().await;
        tokio::time::timeout(std::time::Duration::from_secs(2), finishing)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(store.stored_cancelled().await);
        assert!(matches!(
            source.original_disposition(&namespace, &work),
            Ok(OriginalDisposition::OperationCancelled { .. }),
        ));
        // The private JoinSet, unlike a dropped direct Rust future, published
        // the observed result after the owned SQLite worker completed. No
        // callback/native admission, operation execution or run stop is claimed.
    }

    #[tokio::test]
    async fn eof_waits_for_the_last_real_parent_writer() {
        let (pipes, command_file, response_file) = pipes();
        let last_writer = command_file.try_clone().unwrap();
        let mut commands = tokio::net::unix::pipe::Sender::from_file(command_file).unwrap();
        let _responses = tokio::net::unix::pipe::Receiver::from_file(response_file).unwrap();
        send(
            &mut commands,
            1,
            Command::Bootstrap {
                selection: selection(),
            },
        )
        .await;
        let (owner, connection) =
            PrivateHostConnection::bootstrap(pipes, |_| Ok(ProbeOwner::new(true)))
                .await
                .unwrap();
        drop(commands);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                connection.wait_closed()
            )
            .await
            .is_err()
        );
        assert!(!owner.lost.load(Ordering::Acquire));
        drop(last_writer);
        tokio::time::timeout(std::time::Duration::from_secs(2), connection.wait_closed())
            .await
            .unwrap();
        assert!(owner.lost.load(Ordering::Acquire));
        connection.finish().await.unwrap();
    }

    #[tokio::test]
    async fn failed_bootstrap_response_joins_the_actual_close_before_returning() {
        let (pipes, command_file, response_file) = pipes();
        let mut commands = tokio::net::unix::pipe::Sender::from_file(command_file).unwrap();
        send(
            &mut commands,
            1,
            Command::Bootstrap {
                selection: selection(),
            },
        )
        .await;
        drop(response_file);
        let probe = ProbeOwner::new(false);
        let factory_probe = probe.clone();
        let mut bootstrap = tokio::spawn(PrivateHostConnection::bootstrap(pipes, move |_| {
            Ok(factory_probe)
        }));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while probe.joining.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(probe.lost.load(Ordering::Acquire));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut bootstrap)
                .await
                .is_err()
        );
        probe.settle();
        assert!(bootstrap.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn dropping_connection_waiter_retains_native_join_until_settlement() {
        let (pipes, command_file, response_file) = pipes();
        let mut commands = tokio::net::unix::pipe::Sender::from_file(command_file).unwrap();
        let _responses = tokio::net::unix::pipe::Receiver::from_file(response_file).unwrap();
        send(
            &mut commands,
            1,
            Command::Bootstrap {
                selection: selection(),
            },
        )
        .await;
        let (owner, connection) =
            PrivateHostConnection::bootstrap(pipes, |_| Ok(ProbeOwner::new(false)))
                .await
                .unwrap();
        let owner_observer = Arc::downgrade(&owner);
        drop(connection);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while owner.joining.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(owner.lost.load(Ordering::Acquire));
        owner.settle();
        drop(owner);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while owner_observer.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    fn controller_review_submission() -> PrivateOriginalSubmit {
        use meerkat_authorization_contracts::evidence::EvidenceId;
        PrivateOriginalSubmit {
            original: PrivateOriginalRef {
                route: Id::new("fixture-route").unwrap(),
                original: EvidenceId::new("fixture-original").unwrap(),
                event_sha256: "1".repeat(64),
            },
            content: "fixture input".into(),
            transport_authentication: crate::private_originals::PrivateSourceEvidence {
                resource_id: EvidenceId::new("fixture-auth").unwrap(),
                revision: EvidenceId::new("1").unwrap(),
                sha256: "2".repeat(64),
            },
        }
    }

    #[test]
    fn controller_review_command_cannot_supply_operator_or_human_decision() {
        let request = Request { version: 1, id: 2, command: Command::ReviewControllerParent {
            submission: controller_review_submission(),
        }};
        let value = serde_json::to_value(request).unwrap();
        for field in ["operator", "approved", "lineage", "controller", "viewer"] {
            let mut supplied = value.clone();
            supplied["command"][field] = serde_json::json!(true);
            assert!(serde_json::from_value::<Request>(supplied).is_err());
        }
    }

    #[tokio::test]
    async fn absent_local_presenter_refuses_review_without_closing_other_commands() {
        let (pipes, command_file, response_file) = pipes();
        let mut commands = tokio::net::unix::pipe::Sender::from_file(command_file).unwrap();
        let mut responses = BufReader::new(tokio::net::unix::pipe::Receiver::from_file(response_file).unwrap());
        let mut connection = None;
        let scenario = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            send(&mut commands, 1, Command::Bootstrap { selection: selection() }).await;
            let (_, opened) = PrivateHostConnection::bootstrap(pipes, |_| Ok(ProbeOwner::new(true)))
                .await.map_err(|error| error.to_string())?;
            connection = Some(opened);
            let mut line = String::new();
            responses.read_line(&mut line).await.map_err(|error| error.to_string())?;
            send(&mut commands, 2, Command::ReviewControllerParent { submission: controller_review_submission() }).await;
            line.clear(); responses.read_line(&mut line).await.map_err(|error| error.to_string())?;
            let refused: serde_json::Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
            send(&mut commands, 3, Command::OpenComponent { component: Id::new("existing").unwrap() }).await;
            line.clear(); responses.read_line(&mut line).await.map_err(|error| error.to_string())?;
            let component: serde_json::Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
            Ok::<_, String>((refused, component))
        }).await;
        // Close endpoints and attempt bounded owner cleanup before assertions.
        drop(commands);
        let cleanup = match connection.take() {
            Some(connection) => Some(tokio::time::timeout(std::time::Duration::from_secs(2), connection.finish()).await),
            None => None,
        };
        drop(responses);
        let (refused, component) = scenario.expect("private review scenario timed out").unwrap();
        cleanup.expect("bootstrap did not return a connection").expect("private cleanup timed out").unwrap();
        assert_eq!(refused["id"], 2);
        assert_eq!(refused["result"]["error"], "unavailable");
        assert_eq!(component["result"]["kind"], "component");
    }

    #[tokio::test]
    async fn private_review_is_retained_after_parent_eof_until_its_presenter_settles() {
        // Transport custody test only. This response is not native issuance.
        let (pipes, command_file, response_file) = pipes();
        let mut commands = Some(tokio::net::unix::pipe::Sender::from_file(command_file).unwrap());
        let mut responses = BufReader::new(tokio::net::unix::pipe::Receiver::from_file(response_file).unwrap());
        let (send_review, mut reviews) = mpsc::channel(1);
        let mut connection = None;
        let mut held = None;
        let mut finish = None;
        let scenario = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            send(commands.as_mut().unwrap(), 1, Command::Bootstrap { selection: selection() }).await;
            let (owner, _, opened) = PrivateHostConnection::bootstrap_composed(
                pipes, |_| async { Ok((ProbeOwner::new(true), None)) }, Some(send_review), None,
            ).await.map_err(|error| error.to_string())?;
            connection = Some(opened);
            let mut line = String::new(); responses.read_line(&mut line).await.map_err(|error| error.to_string())?;
            send(commands.as_mut().unwrap(), 2, Command::ReviewControllerParent { submission: controller_review_submission() }).await;
            held = reviews.recv().await;
            let route = held.as_ref().ok_or("review receiver closed")?.submission.original.route.clone();
            drop(commands.take());
            connection.as_ref().unwrap().wait_closed().await;
            let lost = owner.lost.load(Ordering::Acquire);
            let closed = owner.validate_current_selection().is_err();
            finish = Some(tokio::spawn(connection.take().unwrap().finish()));
            // Observe without consuming a completed JoinHandle; cleanup always joins it once.
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let pending = !finish.as_ref().unwrap().is_finished();
            Ok::<_, String>((route, lost, closed, pending))
        }).await;
        // Release the held response unconditionally, even if an earlier
        // observation timed out, before any assertion or final owner join.
        let released = held.take().map(|held| held.response.send(Ok(OperatorParentOutcome::Denied)).is_ok());
        drop(commands.take());
        drop(reviews);
        if finish.is_none() {
            finish = connection.take().map(|connection| tokio::spawn(connection.finish()));
        }
        let cleanup = if let Some(mut task) = finish {
            match tokio::time::timeout(std::time::Duration::from_secs(2), &mut task).await {
                Ok(result) => Some(result),
                Err(_) => {
                    task.abort();
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;
                    None
                }
            }
        } else { None };
        drop(responses);
        let (route, lost, closed, pending) = scenario.expect("private EOF scenario timed out").unwrap();
        cleanup.expect("private EOF cleanup timed out").unwrap().unwrap();
        assert_eq!(route, Id::new("fixture-route").unwrap());
        assert!(lost && closed);
        assert!(pending, "private close discarded the retained review");
        assert_eq!(released, Some(true));
    }

    #[test]
    fn command_vocabulary_has_no_public_rpc_or_request_permission_door() {
        for bytes in [
            br#"{"jsonrpc":"2.0","id":1,"method":"host/bootstrap","params":{}}"#.as_slice(),
            br#"{"version":1,"id":1,"command":{"kind":"enter","requester":"admin"}}"#.as_slice(),
            br#"{"version":1,"id":1,"command":{"kind":"open_component","component":"a","allow":true}}"#.as_slice(),
        ] { assert!(serde_json::from_slice::<Request>(bytes).is_err()); }
    }

    #[tokio::test]
    async fn parent_eof_closes_admission_while_an_existing_close_wait_is_pending() {
        let (pipes, command_file, response_file) = pipes();
        let mut commands = tokio::net::unix::pipe::Sender::from_file(command_file).unwrap();
        let _responses = tokio::net::unix::pipe::Receiver::from_file(response_file).unwrap();
        send(
            &mut commands,
            1,
            Command::Bootstrap {
                selection: selection(),
            },
        )
        .await;
        let (owner, connection) =
            PrivateHostConnection::bootstrap(pipes, |_| Ok(ProbeOwner::new(false)))
                .await
                .unwrap();
        send(
            &mut commands,
            2,
            Command::JoinClose {
                handle: owner.close.clone(),
            },
        )
        .await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while owner.joining.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(commands);
        tokio::time::timeout(std::time::Duration::from_secs(2), connection.wait_closed())
            .await
            .unwrap();
        assert!(owner.lost.load(Ordering::Acquire));
        assert!(
            owner
                .open_component(&Id::new("component").unwrap())
                .is_err()
        );
        let finish = tokio::spawn(connection.finish());
        tokio::task::yield_now().await;
        assert!(
            !finish.is_finished(),
            "transport EOF is not physical settlement"
        );
        owner.settle();
        finish.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn cancelled_read_observation_preserves_partial_frame() {
        let (mut writer, reader) = tokio::io::duplex(1024);
        let mut reader = BufReader::new(reader);
        let mut frame = Vec::new();
        writer.write_all(b"{\"version\":1,").await.unwrap();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                read_request(&mut reader, &mut frame)
            )
            .await
            .is_err()
        );
        assert_eq!(frame, b"{\"version\":1,");
        writer
            .write_all(
                b"\"id\":2,\"command\":{\"kind\":\"open_component\",\"component\":\"fixture\"}}\n",
            )
            .await
            .unwrap();
        let request = read_request(&mut reader, &mut frame)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(request.id, 2);
        assert!(frame.is_empty());
    }

    #[tokio::test]
    async fn malformed_or_oversize_frame_refuses_without_owner_dispatch() {
        // Whitespace keeps the command valid while exercising the complete
        // frame boundary without inventing an oversized application value.
        for extra in [0, 1] {
            let mut bytes = serde_json::to_vec(&serde_json::json!({
                "version": PRIVATE_HOST_PROTOCOL_VERSION, "id": 2,
                "command": {"kind": "open_component", "component": "fixture"},
            })).unwrap();
            bytes.resize(MAX_FRAME_BYTES as usize - 1 + extra, b' ');
            bytes.push(b'\n');
            let (mut writer, reader) = tokio::io::duplex((MAX_FRAME_BYTES + 1) as usize);
            let mut reader = BufReader::new(reader);
            writer.write_all(&bytes).await.unwrap();
            let mut frame = Vec::new();
            let request = read_request(&mut reader, &mut frame).await;
            if extra == 0 {
                assert_eq!(request.unwrap().unwrap().id, 2);
                assert!(frame.is_empty());
            } else {
                assert!(request.is_err());
            }
        }
        let (mut writer, reader) = tokio::io::duplex((MAX_FRAME_BYTES + 1) as usize);
        let mut reader = BufReader::new(reader);
        writer
            .write_all(&vec![b'x'; (MAX_FRAME_BYTES + 1) as usize])
            .await
            .unwrap();
        assert!(read_request(&mut reader, &mut Vec::new()).await.is_err());
    }

    #[tokio::test]
    async fn duplicate_request_correlation_closes_owner_instead_of_replaying_mutation() {
        let (pipes, command_file, response_file) = pipes();
        let mut commands = tokio::net::unix::pipe::Sender::from_file(command_file).unwrap();
        let _responses = tokio::net::unix::pipe::Receiver::from_file(response_file).unwrap();
        send(
            &mut commands,
            1,
            Command::Bootstrap {
                selection: selection(),
            },
        )
        .await;
        let (owner, connection) =
            PrivateHostConnection::bootstrap(pipes, |_| Ok(ProbeOwner::new(true)))
                .await
                .unwrap();
        send(
            &mut commands,
            2,
            Command::OpenComponent {
                component: Id::new("one").unwrap(),
            },
        )
        .await;
        send(
            &mut commands,
            2,
            Command::OpenComponent {
                component: Id::new("two").unwrap(),
            },
        )
        .await;
        tokio::time::timeout(std::time::Duration::from_secs(2), connection.wait_closed())
            .await
            .unwrap();
        assert!(owner.lost.load(Ordering::Acquire));
        assert!(connection.finish().await.is_err());
    }
    // These use the existing transport probe, not a native held compiler. The
    // real driver must separately prove that only actual finish reaches this reply.
    #[tokio::test]
    async fn preparation_reply_and_final_adoption_use_one_reader_and_distinct_requests() {
        let (pipes, command_file, response_file) = pipes();
        let mut commands = tokio::net::unix::pipe::Sender::from_file(command_file).unwrap();
        let mut responses = BufReader::new(tokio::net::unix::pipe::Receiver::from_file(response_file).unwrap());
        let artifact = SelectedHostArtifact { path: "/component-control/plan.json".into(), sha256: "a".repeat(64) };
        send(&mut commands, 1, Command::PrepareApplication { plan: artifact.clone() }).await;
        let startup = PrivateHostStartup::read(pipes).await.unwrap();
        assert!(matches!(startup.intent(), PrivateHostStartupIntent::PrepareApplication(plan) if plan == &artifact));
        let next = tokio::spawn(startup.application_prepared(artifact.clone()));
        let mut line = String::new();
        responses.read_line(&mut line).await.unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"]["kind"], "application_prepared");
        assert_eq!(reply["result"]["manifest"], serde_json::to_value(&artifact).unwrap());
        assert!(!next.is_finished(), "final adoption still needs request 2");
        send(&mut commands, 2, Command::Bootstrap { selection: selection() }).await;
        let startup = next.await.unwrap().unwrap();
        let (owner, _, connection) = PrivateHostConnection::bootstrap_started(
            startup, |_| async { Ok((ProbeOwner::new(true), None)) }, None, None,
        ).await.unwrap();
        line.clear(); responses.read_line(&mut line).await.unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], 2);
        assert_eq!(reply["result"]["kind"], "bootstrap_accepted");
        send(&mut commands, 3, Command::OpenComponent { component: Id::new("next").unwrap() }).await;
        line.clear(); responses.read_line(&mut line).await.unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], 3);
        drop(commands);
        tokio::time::timeout(std::time::Duration::from_secs(2), connection.wait_closed())
            .await
            .unwrap();
        connection.finish().await.unwrap();
        assert!(owner.lost.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn preparation_refuses_early_bootstrap_without_creating_a_private_owner() {
        let (pipes, command_file, _response_file) = pipes();
        let mut commands = tokio::net::unix::pipe::Sender::from_file(command_file).unwrap();
        let artifact = SelectedHostArtifact { path: "/component-control/plan.json".into(), sha256: "a".repeat(64) };
        send(&mut commands, 1, Command::PrepareApplication { plan: artifact.clone() }).await;
        let mut startup = PrivateHostStartup::read(pipes).await.unwrap();
        send(&mut commands, 2, Command::Bootstrap { selection: selection() }).await;
        assert!(matches!(startup.wait_lost().await, PrivateHostError::Protocol));
        assert!(matches!(startup.intent(), PrivateHostStartupIntent::PrepareApplication(_)));
        assert!(matches!(startup.application_prepared(artifact).await, Err(PrivateHostError::Protocol)));
    }

    #[tokio::test]
    async fn console_retirement_fences_through_full_native_join_capacity_and_keeps_reader_live() {
        let (pipes, command_file, response_file) = pipes();
        let mut commands = tokio::net::unix::pipe::Sender::from_file(command_file).unwrap();
        let mut responses = BufReader::new(tokio::net::unix::pipe::Receiver::from_file(response_file).unwrap());
        send(&mut commands, 1, Command::Bootstrap { selection: selection() }).await;
        let (owner, connection) = PrivateHostConnection::bootstrap(pipes, |_| Ok(ProbeOwner::new(false))).await.unwrap();
        let mut line = String::new();
        responses.read_line(&mut line).await.unwrap();
        let expiry = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64 + 60_000;
        send(&mut commands, 2, Command::EnrollConsoleLogin { login: ConsoleLoginSelection {
            issuer: "issuer".into(), subject: "sub".into(), login_scope_id: "a".into(), expires_at_ms: expiry,
        }}).await;
        line.clear(); responses.read_line(&mut line).await.unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["result"]["enrollment_id"], 2);
        let retained = connection.console_logins().resolve("issuer", "sub", "a").unwrap();
        let reservation = retained.reserve().unwrap();
        for id in 3..19 { send(&mut commands, id, Command::JoinClose { handle: owner.close.clone() }).await; }
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while owner.joining.load(Ordering::Acquire) < MAX_PENDING_JOINS { tokio::task::yield_now().await; }
        }).await.unwrap();
        send(&mut commands, 19, Command::RetireConsoleLogin { enrollment_id: 2 }).await;
        assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(2), retained.closed()).await.unwrap(), ConsoleLoginRefusal::Retired);
        assert!(matches!(retained.reserve(), Err(ConsoleLoginRefusal::Retired)));
        // The same reader can process another command while this drain is held.
        send(&mut commands, 20, Command::OpenComponent { component: Id::new("next").unwrap() }).await;
        line.clear(); responses.read_line(&mut line).await.unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], 20);
        reservation.settled();
        line.clear(); responses.read_line(&mut line).await.unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], 19);
        assert_eq!(reply["result"]["observation"], serde_json::json!({"fenced":true,"reservations_drained":true,"uncertain":false}));
        owner.settle();
        for _ in 0..MAX_PENDING_JOINS { line.clear(); responses.read_line(&mut line).await.unwrap(); }
        drop(commands);
        connection.finish().await.unwrap();
    }

    #[tokio::test]
    async fn replacement_keeps_request_one_and_same_owner_until_real_private_close() {
        let (pipes, command_file, response_file) = pipes();
        let mut commands = tokio::net::unix::pipe::Sender::from_file(command_file).unwrap();
        let mut responses = BufReader::new(tokio::net::unix::pipe::Receiver::from_file(response_file).unwrap());
        let selected = ApplicationUpdateStartup {
            selection: SelectedHostArtifact { path: "/component-control/update.json".into(), sha256: "a".repeat(64) },
            prior_tool_policy: SelectedHostArtifact { path: "/component-control/prior.json".into(), sha256: "b".repeat(64) },
            association_directory: "/component-control/output".into(),
        };
        send(&mut commands, 1, Command::ReplaceApplication { startup: selected.clone() }).await;
        let pending = PrivateHostStartup::read(pipes).await.unwrap();
        assert!(matches!(pending.intent(), PrivateHostStartupIntent::ReplaceApplication(actual) if actual == &selected));
        // This inert protocol fixture stands in only for already completed
        // trusted input validation, never a native update or finish result.
        let pending = pending.replace_with_original(selection()).unwrap();
        let (owner, _, connection) = PrivateHostConnection::bootstrap_started(
            pending, |actual| async move { assert_eq!(actual, selection()); Ok((ProbeOwner::new(true), None)) }, None, None,
        ).await.unwrap();
        let mut line = String::new(); responses.read_line(&mut line).await.unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], 1); assert_eq!(reply["result"], serde_json::json!({"kind":"bootstrap_accepted"}));
        send(&mut commands, 2, Command::OpenComponent { component: Id::new("next").unwrap() }).await;
        line.clear(); responses.read_line(&mut line).await.unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&line).unwrap()["id"], 2);
        // Replacement is initial-only, not an update command on a live owner.
        send(&mut commands, 3, Command::ReplaceApplication { startup: selected }).await;
        tokio::time::timeout(std::time::Duration::from_secs(2), connection.wait_closed()).await.unwrap();
        assert!(connection.finish().await.is_err());
        assert!(owner.lost.load(Ordering::Acquire));
    }

}
