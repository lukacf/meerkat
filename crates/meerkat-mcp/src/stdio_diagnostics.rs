//! Setup diagnostics of one stdio MCP server connect attempt (#230): the
//! typed failure class its transport established, and a bounded tail of the
//! server's stderr, held with the attempt's custody record.
//!
//! The router owns each attempt's [`StdioAttemptRecord`] strongly: beside
//! the attempt's process custody until its result is processed, then, for an
//! accepted failure, as that server's one retained failed attempt until the
//! next attempt for it is admitted, its next accepted completion, its
//! removal, its connection's close, shutdown or the router's drop. The
//! process custody, the transport and the stderr drain hold it weakly, so the
//! tail is never kept alive by a process, a close or a clone of the close
//! index.
//!
//! The stderr tail is SENSITIVE: arbitrary server output. It is read only
//! through the host's Rust API, after redaction that is defence in depth
//! only, and is never written into errors, lifecycle notices, events, tool
//! results or logs.

use std::collections::VecDeque;
use std::hash::BuildHasher as _;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use meerkat_core::mcp_attachment::{
    McpStdioExit, McpStdioSetupFailureClass, McpStdioSetupPhase, McpStdioStderrTail,
};
use meerkat_core::redact::SECRET_REDACTION_MARKER;

use crate::connection::{
    McpStdioCloseReceipt, McpStdioLeaderExitObservation, McpStdioLeaderOutcome,
};

/// The most stderr bytes one attempt retains: the newest ones.
pub(crate) const STDERR_TAIL_CAP_BYTES: usize = 16 * 1024;

/// How long the stderr drain keeps reading (and forwarding) after the
/// process it captured is no longer held (its close finished, or its custody
/// dropped it), to collect the last bytes already in the pipe. It then stops
/// and closes its end, so a descendant that keeps the pipe open cannot keep
/// the drain alive; such a descendant then gets `EPIPE` (or `SIGPIPE`) when
/// it writes to stderr.
pub(crate) const STDERR_DRAIN_GRACE: Duration = Duration::from_millis(200);

/// The most launch environment values one attempt tracks for exact-value
/// redaction (each line of a multi-line value counts as one more). Values
/// are taken in [`launch_values_by_priority`] order, so resolved secrets and
/// the connector binding come first; values past this cap are not tracked,
/// and shape redaction still applies to them.
pub(crate) const MAX_TRACKED_SECRET_VALUES: usize = 64;

/// Launch environment values shorter than this are not tracked: they are
/// too short to be told apart from ordinary output.
pub(crate) const MIN_TRACKED_SECRET_BYTES: usize = 8;

/// One read of the stderr drain.
const STDERR_READ_CHUNK_BYTES: usize = 4096;

/// Why the protocol transport stopped receiving from the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportEnd {
    /// The server's stdout reached end of stream.
    Eof,
    /// A frame could not be read: unreadable or over the frame bound.
    BadFrame,
    /// A parse-error reply to the server could not be written.
    ReplyWriteFailed,
}

/// The setup diagnostics of one stdio connect attempt.
pub(crate) struct StdioAttemptRecord {
    state: Mutex<RecordState>,
}

struct RecordState {
    phase: McpStdioSetupPhase,
    failure: Option<(McpStdioSetupPhase, McpStdioSetupFailureClass)>,
    transport_end: Option<TransportEnd>,
    /// Present once the process was spawned with its stderr captured.
    capture: Option<StderrCapture>,
    secrets: SecretDigests,
}

impl std::fmt::Debug for StdioAttemptRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.lock();
        f.debug_struct("StdioAttemptRecord")
            .field("phase", &state.phase)
            .field("failure", &state.failure)
            .field("captured", &state.capture.is_some())
            .finish_non_exhaustive()
    }
}

impl StdioAttemptRecord {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(RecordState {
                phase: McpStdioSetupPhase::Launch,
                failure: None,
                transport_end: None,
                capture: None,
                secrets: SecretDigests::new(),
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RecordState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The process exists with its stderr captured: the handshake starts.
    pub(crate) fn mark_spawned(&self) {
        let mut state = self.lock();
        state.phase = McpStdioSetupPhase::Initialize;
        if state.capture.is_none() {
            state.capture = Some(StderrCapture::default());
        }
    }

    /// The handshake completed: tool discovery starts.
    pub(crate) fn mark_initialized(&self) {
        self.lock().phase = McpStdioSetupPhase::ToolDiscovery;
    }

    #[cfg(test)]
    pub(crate) fn phase(&self) -> McpStdioSetupPhase {
        self.lock().phase
    }

    /// Record the class the transport established, in the current phase.
    /// The first class recorded stands.
    pub(crate) fn classify(&self, class: McpStdioSetupFailureClass) {
        let mut state = self.lock();
        if state.failure.is_none() {
            state.failure = Some((state.phase, class));
        }
    }

    /// The failure recorded so far, with its phase.
    pub(crate) fn failure(&self) -> Option<(McpStdioSetupPhase, McpStdioSetupFailureClass)> {
        self.lock().failure
    }

    /// The first reason the transport stopped receiving stands.
    pub(crate) fn record_transport_end(&self, end: TransportEnd) {
        let mut state = self.lock();
        if state.transport_end.is_none() {
            state.transport_end = Some(end);
        }
    }

    pub(crate) fn transport_end(&self) -> Option<TransportEnd> {
        self.lock().transport_end
    }

    /// Complete a `Closed` class with the server's own exit, when the close
    /// receipt proves it ([`own_exit`]).
    pub(crate) fn attach_exit(&self, receipt: &McpStdioCloseReceipt) {
        let Some(exit) = own_exit(receipt) else {
            return;
        };
        let mut state = self.lock();
        if let Some((_, McpStdioSetupFailureClass::Closed { exit: slot @ None })) =
            state.failure.as_mut()
        {
            *slot = Some(exit);
        }
    }

    /// A failed attempt whose process existed but whose failure the
    /// transport did not classify is `Unknown` in its phase. One that never
    /// spawned keeps no class: its typed error says why.
    pub(crate) fn finish_failed(&self) {
        let mut state = self.lock();
        if state.failure.is_none() && state.capture.is_some() {
            state.failure = Some((state.phase, McpStdioSetupFailureClass::Unknown));
        }
    }

    /// Whether stderr was captured for this attempt (its process spawned).
    pub(crate) fn captured(&self) -> bool {
        self.lock().capture.is_some()
    }

    pub(crate) fn append_stderr(&self, chunk: &[u8]) {
        if let Some(capture) = self.lock().capture.as_mut() {
            capture.extend(chunk);
        }
    }

    /// The drain stopped: at end of stream (`complete`) or at its bound.
    pub(crate) fn stderr_stopped(&self, complete: bool) {
        if let Some(capture) = self.lock().capture.as_mut()
            && capture.stopped.is_none()
        {
            capture.stopped = Some(complete);
        }
    }

    /// Track the launch environment values for exact-value redaction, in
    /// the given order (see [`launch_values_by_priority`]): a keyed digest
    /// of each, and of each line of a multi-line value, never the value
    /// itself.
    pub(crate) fn track_launch_values<'a>(&self, values: impl IntoIterator<Item = &'a str>) {
        self.lock().secrets.track(values);
    }

    /// The redacted tail, while stderr was captured. SENSITIVE.
    pub(crate) fn stderr_tail(&self) -> Option<McpStdioStderrTail> {
        let (bytes, total, complete, secrets) = {
            let state = self.lock();
            let capture = state.capture.as_ref()?;
            (
                capture.ring.iter().copied().collect::<Vec<u8>>(),
                capture.total,
                capture.stopped == Some(true),
                state.secrets.clone(),
            )
        };
        let truncated = total > bytes.len() as u64;
        Some(McpStdioStderrTail::new(
            render_tail(&bytes, truncated, &secrets),
            total,
            truncated,
            complete,
        ))
    }
}

/// The newest stderr bytes, bounded by [`STDERR_TAIL_CAP_BYTES`].
#[derive(Default)]
struct StderrCapture {
    ring: VecDeque<u8>,
    total: u64,
    /// `Some(true)` at end of stream, `Some(false)` when the drain stopped
    /// at its bound.
    stopped: Option<bool>,
}

impl StderrCapture {
    fn extend(&mut self, chunk: &[u8]) {
        self.total = self.total.saturating_add(chunk.len() as u64);
        if chunk.len() >= STDERR_TAIL_CAP_BYTES {
            self.ring.clear();
            self.ring
                .extend(&chunk[chunk.len() - STDERR_TAIL_CAP_BYTES..]);
            return;
        }
        let overflow = (self.ring.len() + chunk.len()).saturating_sub(STDERR_TAIL_CAP_BYTES);
        self.ring.drain(..overflow);
        self.ring.extend(chunk);
    }
}

/// Exact-value redaction state: keyed digests of the attempt's launch
/// environment values. No value is retained: each is kept as its length, a
/// rolling hash (to find candidate windows in linear time per tracked
/// length) and a keyed SipHash (to confirm them), both under keys drawn for
/// this record.
#[derive(Clone)]
struct SecretDigests {
    keys: std::collections::hash_map::RandomState,
    base: u64,
    values: Vec<TrackedValue>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TrackedValue {
    len: usize,
    rolling: u64,
    keyed: u64,
}

/// The Mersenne prime 2^61 - 1, the rolling hash's modulus.
const ROLLING_MODULUS: u64 = (1 << 61) - 1;

fn mul_mod(a: u64, b: u64) -> u64 {
    // The remainder is below the modulus, so it always fits.
    u64::try_from(u128::from(a) * u128::from(b) % u128::from(ROLLING_MODULUS)).unwrap_or(0)
}

fn add_mod(a: u64, b: u64) -> u64 {
    let sum = a + b;
    if sum >= ROLLING_MODULUS {
        sum - ROLLING_MODULUS
    } else {
        sum
    }
}

fn sub_mod(a: u64, b: u64) -> u64 {
    if a >= b {
        a - b
    } else {
        a + ROLLING_MODULUS - b
    }
}

fn rolling_hash(base: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(0, |hash, byte| {
        add_mod(mul_mod(hash, base), u64::from(*byte) + 1)
    })
}

fn pow_mod(base: u64, exponent: usize) -> u64 {
    let mut result = 1;
    let mut square = base;
    let mut exponent = exponent;
    while exponent > 0 {
        if exponent & 1 == 1 {
            result = mul_mod(result, square);
        }
        square = mul_mod(square, square);
        exponent >>= 1;
    }
    result
}

impl SecretDigests {
    fn new() -> Self {
        let keys = std::collections::hash_map::RandomState::new();
        // A base in [2, modulus - 2], drawn from this record's keys.
        let base = keys.hash_one(0x5eed_u64) % (ROLLING_MODULUS - 3) + 2;
        Self {
            keys,
            base,
            values: Vec::new(),
        }
    }

    fn track<'a>(&mut self, values: impl IntoIterator<Item = &'a str>) {
        for value in values {
            // A multi-line value is also tracked line by line, so a value cut
            // at the tail's edge does not show its remaining lines.
            let lines = value
                .contains('\n')
                .then(|| value.lines().map(|line| line.trim_end_matches('\r')))
                .into_iter()
                .flatten();
            for candidate in std::iter::once(value).chain(lines) {
                if !self.track_one(candidate.as_bytes()) {
                    return;
                }
            }
        }
    }

    /// Track one value; `false` once the cap is reached.
    fn track_one(&mut self, bytes: &[u8]) -> bool {
        // Too short to tell apart, or too long to fit the tail whole.
        if bytes.len() < MIN_TRACKED_SECRET_BYTES || bytes.len() > STDERR_TAIL_CAP_BYTES {
            return true;
        }
        let tracked = TrackedValue {
            len: bytes.len(),
            rolling: rolling_hash(self.base, bytes),
            keyed: self.keys.hash_one(bytes),
        };
        if self.values.contains(&tracked) {
            return true;
        }
        if self.values.len() >= MAX_TRACKED_SECRET_VALUES {
            return false;
        }
        self.values.push(tracked);
        true
    }

    /// The byte ranges of `text` equal to a tracked value. Linear in the
    /// text per distinct tracked length (at most
    /// [`MAX_TRACKED_SECRET_VALUES`] of them), plus one keyed hash per
    /// candidate window.
    fn matches(&self, text: &[u8]) -> Vec<(usize, usize)> {
        let mut lengths: Vec<usize> = self.values.iter().map(|value| value.len).collect();
        lengths.sort_unstable();
        lengths.dedup();
        let mut ranges = Vec::new();
        for len in lengths {
            if len == 0 || len > text.len() {
                continue;
            }
            let candidates: Vec<&TrackedValue> = self
                .values
                .iter()
                .filter(|value| value.len == len)
                .collect();
            let leading = pow_mod(self.base, len - 1);
            let mut hash = rolling_hash(self.base, &text[..len]);
            let mut start = 0;
            loop {
                if candidates.iter().any(|value| value.rolling == hash) {
                    let keyed = self.keys.hash_one(&text[start..start + len]);
                    if candidates
                        .iter()
                        .any(|value| value.rolling == hash && value.keyed == keyed)
                    {
                        ranges.push((start, start + len));
                    }
                }
                if start + len >= text.len() {
                    break;
                }
                let outgoing = u64::from(text[start]) + 1;
                let incoming = u64::from(text[start + len]) + 1;
                hash = add_mod(
                    mul_mod(sub_mod(hash, mul_mod(outgoing, leading)), self.base),
                    incoming,
                );
                start += 1;
            }
        }
        ranges
    }

    /// `text` with every tracked value replaced by the redaction marker.
    fn redact(&self, text: &[u8]) -> Vec<u8> {
        let mut ranges = self.matches(text);
        if ranges.is_empty() {
            return text.to_vec();
        }
        ranges.sort_unstable();
        let mut output = Vec::with_capacity(text.len());
        let mut cursor = 0;
        for (start, end) in ranges {
            if end <= cursor {
                continue;
            }
            if start >= cursor {
                output.extend_from_slice(&text[cursor..start]);
                output.extend_from_slice(SECRET_REDACTION_MARKER.as_bytes());
            }
            // Overlapping a range already redacted: extend it.
            cursor = end;
        }
        output.extend_from_slice(&text[cursor..]);
        output
    }
}

/// Where a truncated tail may start: after the first line break, or, with
/// none, after the first whitespace; nothing when there is neither, so no
/// partial line or token cut at the edge is shown.
fn cut_start(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .or_else(|| bytes.iter().position(u8::is_ascii_whitespace))
        .map_or(bytes.len(), |index| index + 1)
}

fn render_tail(bytes: &[u8], truncated: bool, secrets: &SecretDigests) -> String {
    let start = if truncated { cut_start(bytes) } else { 0 };
    let exact = secrets.redact(&bytes[start..]);
    let text = strip_controls(&String::from_utf8_lossy(&exact));
    // A private key cut at the edge starts the tail inside its body: with
    // no BEGIN marker before its END marker, everything up to that END
    // marker is redacted.
    let text = match truncated.then(|| leading_private_key_end(&text)).flatten() {
        Some(end) => format!("{SECRET_REDACTION_MARKER}{}", &text[end..]),
        None => text,
    };
    meerkat_core::redact::redact_secret_shapes(&text)
}

/// `text` without control characters (C0, DEL and C1) other than line feed
/// and tab, so the tail cannot carry terminal escape sequences, carriage
/// returns or other control codes to whoever prints it.
fn strip_controls(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect()
}

/// The end of the first private-key END marker of `text`, when no BEGIN
/// marker precedes it.
fn leading_private_key_end(text: &str) -> Option<usize> {
    let mut search = 0;
    while let Some(found) = text[search..].find("-----END ") {
        let start = search + found;
        let label_start = start + "-----END ".len();
        let label_len = text[label_start..].find("-----")?;
        let label = &text[label_start..label_start + label_len];
        let end = label_start + label_len + "-----".len();
        if !label.contains('\n') && label.ends_with("PRIVATE KEY") {
            return (!text[..start].contains("-----BEGIN ")).then_some(end);
        }
        search = end;
    }
    None
}

/// A launch configuration's environment values in tracking order: the
/// resolved secrets (`secret_env`) first, then the connector binding, then
/// every other value by name. The cap ([`MAX_TRACKED_SECRET_VALUES`]) is
/// applied in this order.
pub(crate) fn launch_values_by_priority(
    stdio: &meerkat_core::mcp_config::McpStdioConfig,
) -> Vec<&str> {
    let binding = meerkat_core::mcp_config::MCP_CONNECTOR_BINDING_ENV;
    let mut ordered: Vec<&str> = stdio
        .secret_env
        .iter()
        .filter_map(|name| stdio.env.get(name).map(String::as_str))
        .collect();
    if let Some(value) = stdio.env.get(binding) {
        ordered.push(value);
    }
    let mut rest: Vec<(&String, &String)> = stdio
        .env
        .iter()
        .filter(|(name, _)| !stdio.secret_env.contains(*name) && name.as_str() != binding)
        .collect();
    rest.sort_unstable_by(|left, right| left.0.cmp(right.0));
    ordered.extend(rest.into_iter().map(|(_, value)| value.as_str()));
    ordered
}

/// The server's own exit, when the close receipt proves the process exited
/// before the close escalated: on Linux and Android seen to exit (without
/// reaping) during the EOF phase with no signal sent; off Unix reaped during
/// the EOF phase with no kill. Elsewhere (macOS) the exit cannot be observed
/// without reaping, so it is not claimed.
pub(crate) fn own_exit(receipt: &McpStdioCloseReceipt) -> Option<McpStdioExit> {
    let McpStdioLeaderOutcome::Reaped(status) = &receipt.leader else {
        return None;
    };
    if receipt.sigterm.is_some() || receipt.sigkill.is_some() {
        return None;
    }
    let proven = match receipt.leader_exit {
        McpStdioLeaderExitObservation::Exited(crate::connection::McpStdioClosePhase::Eof) => true,
        McpStdioLeaderExitObservation::Unobservable => {
            cfg!(not(unix))
                && receipt.settled_in == Some(crate::connection::McpStdioClosePhase::Eof)
        }
        _ => false,
    };
    if !proven {
        return None;
    }
    exit_of(status)
}

fn exit_of(status: &std::process::ExitStatus) -> Option<McpStdioExit> {
    if let Some(code) = status.code() {
        return Some(McpStdioExit::Code(code));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        status.signal().map(McpStdioExit::Signal)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// The class an initialize failure establishes, given why the transport
/// stopped receiving. Nothing is guessed: what rmcp and the transport do not
/// tell apart is `Unknown`.
pub(crate) fn classify_initialize_error(
    error: &rmcp::service::ClientInitializeError,
    end: Option<TransportEnd>,
) -> McpStdioSetupFailureClass {
    use rmcp::service::ClientInitializeError as E;
    match error {
        E::ConnectionClosed(_) => class_of_end(end),
        E::TransportError { error, .. } if broken_pipe(error) => {
            McpStdioSetupFailureClass::Closed { exit: None }
        }
        E::JsonRpcError(data) => McpStdioSetupFailureClass::Rejected { code: data.code.0 },
        E::ExpectedInitResult(_) | E::ExpectedInitResponse(_) | E::ConflictInitResponseId(..) => {
            McpStdioSetupFailureClass::ProtocolError
        }
        _ => McpStdioSetupFailureClass::Unknown,
    }
}

/// The class a failed request after the handshake (tool discovery)
/// establishes.
pub(crate) fn classify_service_error(
    error: &rmcp::ServiceError,
    end: Option<TransportEnd>,
) -> McpStdioSetupFailureClass {
    match error {
        rmcp::ServiceError::TransportClosed => class_of_end(end),
        rmcp::ServiceError::TransportSend(error) if broken_pipe(error) => {
            McpStdioSetupFailureClass::Closed { exit: None }
        }
        rmcp::ServiceError::McpError(data) => {
            McpStdioSetupFailureClass::Rejected { code: data.code.0 }
        }
        rmcp::ServiceError::UnexpectedResponse => McpStdioSetupFailureClass::ProtocolError,
        rmcp::ServiceError::Timeout { .. } => McpStdioSetupFailureClass::TimedOut,
        _ => McpStdioSetupFailureClass::Unknown,
    }
}

fn class_of_end(end: Option<TransportEnd>) -> McpStdioSetupFailureClass {
    match end {
        Some(TransportEnd::Eof) => McpStdioSetupFailureClass::Closed { exit: None },
        Some(TransportEnd::BadFrame) => McpStdioSetupFailureClass::ProtocolError,
        Some(TransportEnd::ReplyWriteFailed) | None => McpStdioSetupFailureClass::Unknown,
    }
}

fn broken_pipe(error: &rmcp::transport::DynamicTransportError) -> bool {
    error
        .error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::BrokenPipe)
}

/// The held stderr drain of one process: dropping it (when the process is
/// no longer held) starts the drain's [`STDERR_DRAIN_GRACE`].
pub(crate) struct StderrDrain {
    _held: tokio::sync::oneshot::Sender<()>,
}

/// Drain `stderr` until end of stream on a task of its own, so the server
/// does not block on a full pipe: the newest [`STDERR_TAIL_CAP_BYTES`] go to
/// the attempt's record while it exists, everything else is discarded, and
/// with `forward` every chunk is also written, best effort, to this
/// process's own stderr (as an inherited stderr showed it; while the process
/// is held a blocked host stderr can hold the drain, as it would have held
/// the server). The task holds the record weakly and stops, forward writes
/// included, [`STDERR_DRAIN_GRACE`] after the returned handle is dropped.
pub(crate) fn start_stderr_drain(
    mut stderr: tokio::process::ChildStderr,
    record: Weak<StdioAttemptRecord>,
    forward: bool,
) -> StderrDrain {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let (held, mut released) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let mut buffer = vec![0_u8; STDERR_READ_CHUNK_BYTES];
        let mut host_stderr = forward.then(tokio::io::stderr);
        let mut deadline: Option<tokio::time::Instant> = None;
        let complete = loop {
            let read = match deadline {
                Some(deadline) => {
                    match tokio::time::timeout_at(deadline, stderr.read(&mut buffer)).await {
                        Ok(read) => read,
                        Err(_) => break false,
                    }
                }
                None => {
                    tokio::select! {
                        biased;
                        read = stderr.read(&mut buffer) => read,
                        _ = &mut released => {
                            deadline = Some(tokio::time::Instant::now() + STDERR_DRAIN_GRACE);
                            continue;
                        }
                    }
                }
            };
            match read {
                Ok(0) => break true,
                Err(_) => break false,
                Ok(read) => {
                    let chunk = &buffer[..read];
                    if let Some(record) = record.upgrade() {
                        record.append_stderr(chunk);
                    }
                    if let Some(host) = host_stderr.as_mut() {
                        // Bounded by the same deadline once the process is
                        // no longer held.
                        let write = host.write_all(chunk);
                        tokio::pin!(write);
                        let written = match deadline {
                            Some(deadline) => {
                                tokio::time::timeout_at(deadline, &mut write).await.ok()
                            }
                            None => {
                                tokio::select! {
                                    biased;
                                    written = &mut write => Some(written),
                                    _ = &mut released => {
                                        let until = tokio::time::Instant::now() + STDERR_DRAIN_GRACE;
                                        deadline = Some(until);
                                        tokio::time::timeout_at(until, &mut write).await.ok()
                                    }
                                }
                            }
                        };
                        match written {
                            Some(Ok(())) => {}
                            Some(Err(_)) => host_stderr = None,
                            None => break false,
                        }
                    }
                }
            }
        };
        if let Some(record) = record.upgrade() {
            record.stderr_stopped(complete);
        }
    });
    StderrDrain { _held: held }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn record_with(values: &[&str]) -> Arc<StdioAttemptRecord> {
        let record = StdioAttemptRecord::new();
        record.track_launch_values(values.iter().copied());
        record.mark_spawned();
        record
    }

    #[test]
    fn the_tail_keeps_the_newest_bytes_and_counts_all() {
        let record = record_with(&[]);
        record.append_stderr(&vec![b'a'; STDERR_TAIL_CAP_BYTES]);
        record.append_stderr(b"\nnewest line\n");
        let tail = record.stderr_tail().expect("captured");
        assert!(tail.truncated);
        assert_eq!(
            tail.total_bytes,
            (STDERR_TAIL_CAP_BYTES + "\nnewest line\n".len()) as u64
        );
        // The partial first line at the cut is not shown.
        assert_eq!(tail.text, "newest line\n");
        assert!(!tail.complete);
        record.stderr_stopped(true);
        assert!(record.stderr_tail().expect("captured").complete);
    }

    #[test]
    fn a_chunk_larger_than_the_cap_keeps_its_end() {
        let record = record_with(&[]);
        let mut chunk = vec![b'x'; STDERR_TAIL_CAP_BYTES * 2];
        chunk.extend_from_slice(b" tail-end");
        record.append_stderr(&chunk);
        let tail = record.stderr_tail().expect("captured");
        assert!(tail.truncated);
        assert_eq!(tail.text, "tail-end", "cut after the first whitespace");
    }

    #[test]
    fn an_untruncated_tail_is_shown_whole_and_lossy() {
        let record = record_with(&[]);
        record.append_stderr(b"first\n\xffsecond\n");
        let tail = record.stderr_tail().expect("captured");
        assert!(!tail.truncated);
        assert_eq!(tail.text, "first\n\u{fffd}second\n");
    }

    #[test]
    fn nothing_is_captured_before_the_spawn() {
        let record = StdioAttemptRecord::new();
        record.append_stderr(b"lost");
        assert!(record.stderr_tail().is_none());
        assert!(!record.captured());
    }

    #[test]
    fn launch_values_are_redacted_wherever_they_repeat() {
        let secret = "s3cret-launch-value";
        let record = record_with(&[secret]);
        record.append_stderr(format!("a {secret} b {secret} c\n").as_bytes());
        let tail = record.stderr_tail().expect("captured");
        assert!(!tail.text.contains("s3cret"), "{:?}", tail.text);
        assert_eq!(tail.text, "a [REDACTED] b [REDACTED] c\n");
    }

    #[test]
    fn overlapping_launch_values_become_one_redaction() {
        let record = record_with(&["aaaabbbbcc", "bbbbccdddd"]);
        record.append_stderr(b"x aaaabbbbccdddd y\n");
        let tail = record.stderr_tail().expect("captured");
        assert_eq!(tail.text, "x [REDACTED] y\n");
    }

    #[test]
    fn a_value_shorter_than_the_minimum_is_not_tracked() {
        let record = record_with(&["abc1234"]);
        record.append_stderr(b"value abc1234 shown\n");
        let tail = record.stderr_tail().expect("captured");
        assert_eq!(tail.text, "value abc1234 shown\n");
    }

    #[test]
    fn a_value_straddling_the_cut_shows_no_part_of_it() {
        let secret = "straddling-secret-value";
        let record = record_with(&[secret]);
        // The cut falls inside the secret: its end is the first retained
        // bytes, on the same partial line.
        let mut stream = vec![b'p'; 10];
        stream.extend_from_slice(secret.as_bytes());
        stream.extend_from_slice(b" rest of that line\nnext line\n");
        let keep = STDERR_TAIL_CAP_BYTES;
        let filler = keep - (stream.len() - 10 - 5);
        let mut all = stream.clone();
        all.extend(std::iter::repeat_n(b'f', filler));
        all.push(b'\n');
        record.append_stderr(&all);
        let tail = record.stderr_tail().expect("captured");
        assert!(tail.truncated);
        for part in ["straddling", "secret-value", "-value", "value"] {
            assert!(!tail.text.contains(part), "{part:?} in {:?}", tail.text);
        }
        assert!(
            tail.text.starts_with("next line\n"),
            "{:?}",
            &tail.text[..20]
        );
    }

    #[test]
    fn at_most_the_capped_number_of_values_is_tracked() {
        let values: Vec<String> = (0..=MAX_TRACKED_SECRET_VALUES)
            .map(|index| format!("tracked-value-{index:04}"))
            .collect();
        let record = record_with(&values.iter().map(String::as_str).collect::<Vec<_>>());
        let line = values.join(" ");
        record.append_stderr(line.as_bytes());
        let tail = record.stderr_tail().expect("captured");
        assert_eq!(
            tail.text.matches(SECRET_REDACTION_MARKER).count(),
            MAX_TRACKED_SECRET_VALUES
        );
        assert!(
            tail.text
                .ends_with(&format!("tracked-value-{MAX_TRACKED_SECRET_VALUES:04}")),
            "the value past the cap is not tracked: {:?}",
            tail.text
        );
    }

    /// The tail's newest bytes: `stream`, cut so that the retained bytes
    /// start `cut_into` bytes into `marker`.
    fn cut_inside(marker: &str, stream: &str, cut_into: usize) -> Vec<u8> {
        let keep_from = stream.find(marker).expect("the marker is in the stream") + cut_into;
        let mut all = vec![b'p'; STDERR_TAIL_CAP_BYTES];
        all.extend_from_slice(stream.as_bytes());
        // Enough after the stream that the retained window (the last cap
        // bytes) starts exactly at `keep_from`.
        let suffix = STDERR_TAIL_CAP_BYTES + keep_from - stream.len();
        all.extend(std::iter::repeat_n(b'f', suffix - 1));
        all.push(b'\n');
        all
    }

    #[test]
    fn each_line_of_a_multi_line_launch_value_is_tracked() {
        let value = "first-secret-line-aaaa\nsecond-secret-line-bbbb\nthird-secret-line-cccc";
        let record = record_with(&[value]);
        let stream = format!("echoed:\n{value}\nafter the value\n");
        record.append_stderr(&cut_inside("first-secret-line", &stream, 5));
        let tail = record.stderr_tail().expect("captured");
        assert!(tail.truncated);
        for line in ["first-secret", "second-secret-line", "third-secret-line"] {
            assert!(
                !tail.text.contains(line),
                "{line:?} in {:?}",
                &tail.text[..200]
            );
        }
        assert!(tail.text.contains("after the value"));
    }

    #[test]
    fn a_private_key_cut_at_the_edge_is_redacted_up_to_its_end_marker() {
        let record = record_with(&[]);
        let stream = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASC\nBKcwggSjAgEAAoIBAQC7VJTUt9Us8cKj\n-----END PRIVATE KEY-----\nafter the key\n";
        record.append_stderr(&cut_inside("MIIEvQIBADANBgkq", stream, 4));
        let tail = record.stderr_tail().expect("captured");
        assert!(tail.truncated);
        assert!(
            !tail.text.contains("BKcwggSjAgEAAoIBAQC7VJTUt9Us8cKj"),
            "{:?}",
            &tail.text[..120]
        );
        assert!(!tail.text.contains("-----END"), "{:?}", &tail.text[..120]);
        assert!(
            tail.text.starts_with("[REDACTED]\nafter the key\n"),
            "{:?}",
            &tail.text[..120]
        );

        // The same with no line break at all: the whitespace cut.
        let record = record_with(&[]);
        let mut line = vec![b'q'; STDERR_TAIL_CAP_BYTES];
        line.extend_from_slice(
            b"KEYBODY body-part-two -----END RSA PRIVATE KEY----- trailing words",
        );
        record.append_stderr(&line);
        let tail = record.stderr_tail().expect("captured");
        assert!(tail.truncated);
        assert_eq!(tail.text, "[REDACTED] trailing words");
    }

    #[test]
    fn resolved_secrets_are_tracked_before_the_cap() {
        let mut config = meerkat_core::McpServerConfig::stdio(
            "capped",
            "/bin/true",
            vec![],
            std::collections::HashMap::new(),
        );
        let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) = &mut config.transport
        else {
            panic!("stdio");
        };
        for index in 0..MAX_TRACKED_SECRET_VALUES + 6 {
            stdio.env.insert(
                format!("ORDINARY_{index:03}"),
                format!("ordinary-value-{index:03}"),
            );
        }
        stdio.env.insert(
            "API_SECRET".to_string(),
            "the-resolved-secret-value".to_string(),
        );
        stdio.secret_env.insert("API_SECRET".to_string());
        let ordered = launch_values_by_priority(stdio);
        assert_eq!(ordered.first(), Some(&"the-resolved-secret-value"));
        let record = record_with(&ordered);
        record.append_stderr(b"the-resolved-secret-value ordinary-value-000 ordinary-value-069\n");
        let tail = record.stderr_tail().expect("captured");
        assert_eq!(tail.text, "[REDACTED] [REDACTED] ordinary-value-069\n");
    }

    #[test]
    fn control_characters_other_than_line_feed_and_tab_are_removed() {
        let record = record_with(&[]);
        record.append_stderr("a\x1b[31mred\x1b[0m\x07 bell\r\n\u{9b}c\u{7f}d\tt\n".as_bytes());
        let tail = record.stderr_tail().expect("captured");
        assert_eq!(tail.text, "a[31mred[0m bell\ncd\tt\n");
    }

    #[test]
    fn the_class_is_recorded_once_in_its_phase() {
        let record = StdioAttemptRecord::new();
        assert_eq!(record.phase(), McpStdioSetupPhase::Launch);
        record.finish_failed();
        assert_eq!(record.failure(), None, "never spawned: no class");
        record.mark_spawned();
        record.classify(McpStdioSetupFailureClass::TimedOut);
        record.classify(McpStdioSetupFailureClass::Unknown);
        assert_eq!(
            record.failure(),
            Some((
                McpStdioSetupPhase::Initialize,
                McpStdioSetupFailureClass::TimedOut
            ))
        );
        let record = StdioAttemptRecord::new();
        record.mark_spawned();
        record.mark_initialized();
        record.finish_failed();
        assert_eq!(
            record.failure(),
            Some((
                McpStdioSetupPhase::ToolDiscovery,
                McpStdioSetupFailureClass::Unknown
            ))
        );
    }

    #[test]
    fn initialize_errors_are_classified_only_as_far_as_established() {
        use rmcp::service::ClientInitializeError as E;
        let closed = E::ConnectionClosed("initialize response".into());
        assert_eq!(
            classify_initialize_error(&closed, Some(TransportEnd::Eof)),
            McpStdioSetupFailureClass::Closed { exit: None }
        );
        assert_eq!(
            classify_initialize_error(&closed, Some(TransportEnd::BadFrame)),
            McpStdioSetupFailureClass::ProtocolError
        );
        assert_eq!(
            classify_initialize_error(&closed, None),
            McpStdioSetupFailureClass::Unknown
        );
        let rejected = E::JsonRpcError(rmcp::ErrorData::new(
            rmcp::model::ErrorCode::METHOD_NOT_FOUND,
            "Method not found",
            None,
        ));
        assert_eq!(
            classify_initialize_error(&rejected, None),
            McpStdioSetupFailureClass::Rejected { code: -32601 }
        );
        assert_eq!(
            classify_initialize_error(&E::ExpectedInitResult(None), None),
            McpStdioSetupFailureClass::ProtocolError
        );
        assert_eq!(
            classify_initialize_error(&E::Cancelled, Some(TransportEnd::Eof)),
            McpStdioSetupFailureClass::Unknown
        );
    }
}
