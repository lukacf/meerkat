//! RPC server main loop.
//!
//! Wires together the JSON-RPC transport, method router, and notification
//! channel. Uses `tokio::select!` to multiplex incoming requests, outgoing
//! responses from spawned dispatch tasks, and event notifications.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, OnceLock};

use meerkat::surface::{
    RequestAdmissionError, RequestTerminalResolution, SurfaceRequestExecutor,
    SurfaceRequestSemantics, noop_request_action,
};
use meerkat_contracts::{
    LiveSendInputErrorData, WireLiveAdapterErrorCode, WireLiveConfigRejectionReason,
    rpc_request_lifecycle,
};
use tokio::io::{AsyncBufRead, BufReader};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};

use meerkat_core::ConfigStore;

use crate::NOTIFICATION_CHANNEL_CAPACITY;
use crate::callback_dispatcher::{CallbackRequestEnvelope, CallbackResponseHandoff};
use crate::handlers::RpcResponseExt;
use crate::protocol::{RpcId, RpcMessage, RpcNotification, RpcRequest, RpcResponse};
use crate::router::{MethodRouter, NotificationSink};
use crate::session_runtime::SessionRuntime;
use crate::transport::{
    AdmittedJsonlMessage, BlockingWriter, JSONL_MAX_FRAME_BYTES, JSONL_SMALL_FRAME_BYTES,
    JsonlFrameAdmission, JsonlTransport, TransportError, TransportWriter,
};

/// Conservative multiplier from retained JSON request bytes to peak dispatch
/// memory. Dispatch can temporarily hold the raw params alongside parsed
/// handler input and an encoded response. `live/send_input` additionally holds
/// decoded media/provider projections, but the same multiplier plus its larger
/// minimum charge gives every method one process-owned memory invariant.
const RPC_REQUEST_MEMORY_CHARGE_MULTIPLIER: usize = 4;

/// A valid maximum-size JSONL frame can retain a small amount of Rust envelope
/// metadata beyond its wire bytes. Keep explicit slack so one maximum-valid
/// request is always admissible after applying the peak-memory multiplier.
const RPC_REQUEST_MAX_RETAINED_BYTES: usize = JSONL_MAX_FRAME_BYTES + 4 * 1024;

/// Process-wide request-memory window shared by all connections accepted by
/// one RPC TCP server. This admits one maximum-valid JSONL request while
/// bounding the combined raw/parsed/decoded/response peak for every method.
const RPC_PROCESS_REQUEST_MEMORY_BUDGET_BYTES: usize =
    RPC_REQUEST_MAX_RETAINED_BYTES * RPC_REQUEST_MEMORY_CHARGE_MULTIPLIER;

/// Minimum admission charge for one `live/send_input` request.
///
/// A byte-only budget does not account for task/router/permit overhead: tiny
/// requests could otherwise create millions of independently stalled tasks.
/// Applying the peak-memory multiplier to this floor limits the process-wide
/// window to at most roughly 256 concurrently admitted tiny live inputs.
const LIVE_SEND_INPUT_MIN_ADMISSION_BYTES: usize = 256 * 1024;

/// Minimum admission charge for one `live/open` request.
///
/// A cold or reconnecting open can hydrate two maximum durable images and
/// project them into provider seed events. Reserve the complete process request
/// window before that amplification begins so at most one image-heavy open can
/// execute at a time. Cancellation and callback/outbound traffic retain their
/// independent control-plane budgets.
const LIVE_OPEN_MIN_ADMISSION_BYTES: usize = RPC_REQUEST_MAX_RETAINED_BYTES;

/// Minimum retained-byte charge for an ordinary RPC request. The explicit
/// task semaphore below is the hard count bound; this floor also accounts for
/// task/router/response envelope overhead which is not represented by the
/// request's JSON byte length.
const RPC_REQUEST_MIN_ADMISSION_BYTES: usize = 64 * 1024;

/// Maximum process-wide request tasks admitted across every RPC connection.
/// This is deliberately below the per-connection response channel capacity so
/// response senders cannot accumulate as parked tasks behind a full channel.
const RPC_PROCESS_MAX_INFLIGHT_REQUESTS: usize = 256;

/// Method names select a finite generated/router surface and are reflected in
/// method-not-found responses. Cap them independently from params so a single
/// 64 MiB JSONL frame cannot amplify into another attacker-sized response.
const RPC_MAX_METHOD_BYTES: usize = 256;

/// Cap the echo/control-bearing request envelope independently from raw params.
/// This covers `jsonrpc`, method, id, and Rust envelope metadata. Params retain
/// the transport's 64 MiB frame ceiling and are charged by process admission.
const RPC_MAX_REQUEST_ENVELOPE_BYTES: usize = 4 * 1024;

/// Cancellation bypasses data-plane request admission so it can release work
/// while that admission is saturated. Keep its only payload independently
/// small before serde allocates a target id.
const RPC_MAX_CANCEL_PARAMS_BYTES: usize = 4 * 1024;
const RPC_CANCEL_PROCESS_MEMORY_BUDGET_BYTES: usize = 2 * 1024 * 1024;
const RPC_CANCEL_MAX_INFLIGHT: usize = 128;
const RPC_CANCEL_MAX_RETAINED_BYTES: usize = 8 * 1024;

/// Callback tool responses are client-controlled data handed into an internal
/// oneshot and parsed into a second owned tool result. Cap each handoff well
/// below the general JSONL frame ceiling and reserve both raw-result and parsed
/// projection peaks process-wide until the callback consumer finishes parsing.
const RPC_CALLBACK_RESPONSE_MAX_RETAINED_BYTES: usize = 16 * 1024 * 1024;
const RPC_CALLBACK_RESPONSE_MEMORY_CHARGE_MULTIPLIER: usize = 2;
const RPC_CALLBACK_RESPONSE_MIN_ADMISSION_BYTES: usize = 64 * 1024;
const RPC_CALLBACK_RESPONSE_PROCESS_MEMORY_BUDGET_BYTES: usize = 64 * 1024 * 1024;
const RPC_CALLBACK_RESPONSE_MAX_INFLIGHT: usize = 64;
const RPC_CALLBACK_RESPONSE_MAX_ERROR_MESSAGE_BYTES: usize = 4 * 1024;

/// High process connection ceiling for TCP task/socket overhead. Full-frame
/// memory is owned separately by incremental [`JsonlFrameAdmission`], so a
/// handful of slow or idle clients cannot reserve maximum-frame capacity.
const RPC_TCP_MAX_CONNECTIONS: usize = 128;

/// String JSON-RPC ids are echoed into responses, so cap them before task
/// spawn. The rejection deliberately responds with a null id rather than
/// retaining or echoing the oversized attacker-controlled string.
const RPC_MAX_STRING_ID_BYTES: usize = 1024;

#[derive(Clone)]
struct RpcRequestAdmission {
    memory_semaphore: Arc<Semaphore>,
    task_semaphore: Arc<Semaphore>,
    capacity_bytes: usize,
    max_inflight_requests: usize,
}

#[derive(Debug)]
struct RpcRequestAdmissionPermit {
    _memory_permit: OwnedSemaphorePermit,
    _task_permit: OwnedSemaphorePermit,
}

/// Reserved control-plane custody for cancellation notifications. It is
/// intentionally independent of ordinary request admission so a saturated
/// data plane cannot prevent clients from releasing already-running work.
#[derive(Clone)]
struct RpcCancelAdmission {
    memory_semaphore: Arc<Semaphore>,
    count_semaphore: Arc<Semaphore>,
}

#[derive(Debug)]
struct RpcCancelAdmissionPermit {
    _memory_permit: OwnedSemaphorePermit,
    _count_permit: OwnedSemaphorePermit,
}

impl RpcCancelAdmission {
    fn production() -> Self {
        static PROCESS_ADMISSION: OnceLock<RpcCancelAdmission> = OnceLock::new();
        PROCESS_ADMISSION
            .get_or_init(|| {
                Self::new(
                    RPC_CANCEL_PROCESS_MEMORY_BUDGET_BYTES,
                    RPC_CANCEL_MAX_INFLIGHT,
                )
            })
            .clone()
    }

    fn new(capacity_bytes: usize, max_inflight: usize) -> Self {
        Self {
            memory_semaphore: Arc::new(Semaphore::new(capacity_bytes)),
            count_semaphore: Arc::new(Semaphore::new(max_inflight)),
        }
    }

    fn admit(&self, request: &RpcRequest) -> Option<RpcCancelAdmissionPermit> {
        let retained_bytes = retained_rpc_request_bytes_unchecked(request);
        if retained_bytes > RPC_CANCEL_MAX_RETAINED_BYTES {
            return None;
        }
        let permits = u32::try_from(retained_bytes.max(1)).ok()?;
        let count_permit = Arc::clone(&self.count_semaphore).try_acquire_owned().ok()?;
        let memory_permit = Arc::clone(&self.memory_semaphore)
            .try_acquire_many_owned(permits)
            .ok()?;
        Some(RpcCancelAdmissionPermit {
            _memory_permit: memory_permit,
            _count_permit: count_permit,
        })
    }
}

#[derive(Clone)]
struct RpcCallbackResponseAdmission {
    memory_semaphore: Arc<Semaphore>,
    count_semaphore: Arc<Semaphore>,
    capacity_bytes: usize,
    max_inflight: usize,
    max_retained_bytes: usize,
}

#[derive(Debug, thiserror::Error)]
enum RpcCallbackResponseAdmissionError {
    #[error("callback response id is missing or exceeds the RPC id ceiling")]
    InvalidResponseId,
    #[error("callback response must declare JSON-RPC 2.0")]
    UnsupportedVersion,
    #[error("callback response must contain exactly one of result or error")]
    InvalidEnvelope,
    #[error(
        "callback response error message is too large: {actual_bytes} bytes (maximum {max_bytes})"
    )]
    ErrorMessageTooLarge {
        max_bytes: usize,
        actual_bytes: usize,
    },
    #[error(
        "callback response is too large: {retained_bytes} retained bytes (maximum {max_bytes})"
    )]
    ResponseTooLarge {
        max_bytes: usize,
        retained_bytes: usize,
    },
    #[error(
        "callback response admission is saturated: requested {requested_bytes} bytes from a {max_inflight_bytes}-byte / {max_inflight}-response budget"
    )]
    Backpressured {
        max_inflight_bytes: usize,
        max_inflight: usize,
        requested_bytes: usize,
    },
    #[error("callback response admission permit range exceeded")]
    PermitRange,
    #[error("callback response admission is closed")]
    Closed,
}

impl RpcCallbackResponseAdmissionError {
    fn jsonrpc_code(&self) -> i32 {
        match self {
            Self::Backpressured { .. } => crate::error::BUDGET_EXHAUSTED,
            Self::Closed => crate::error::INTERNAL_ERROR,
            Self::InvalidResponseId
            | Self::UnsupportedVersion
            | Self::InvalidEnvelope
            | Self::ErrorMessageTooLarge { .. }
            | Self::ResponseTooLarge { .. }
            | Self::PermitRange => crate::error::INVALID_REQUEST,
        }
    }
}

impl RpcCallbackResponseAdmission {
    fn production() -> Self {
        static PROCESS_ADMISSION: OnceLock<RpcCallbackResponseAdmission> = OnceLock::new();
        PROCESS_ADMISSION
            .get_or_init(|| {
                Self::new(
                    RPC_CALLBACK_RESPONSE_PROCESS_MEMORY_BUDGET_BYTES,
                    RPC_CALLBACK_RESPONSE_MAX_INFLIGHT,
                    RPC_CALLBACK_RESPONSE_MAX_RETAINED_BYTES,
                )
            })
            .clone()
    }

    fn new(capacity_bytes: usize, max_inflight: usize, max_retained_bytes: usize) -> Self {
        Self {
            memory_semaphore: Arc::new(Semaphore::new(capacity_bytes)),
            count_semaphore: Arc::new(Semaphore::new(max_inflight)),
            capacity_bytes,
            max_inflight,
            max_retained_bytes,
        }
    }

    fn admit(
        &self,
        mut response: RpcResponse,
        wire_bytes: usize,
    ) -> Result<CallbackResponseHandoff, RpcCallbackResponseAdmissionError> {
        if wire_bytes > self.max_retained_bytes {
            return Err(RpcCallbackResponseAdmissionError::ResponseTooLarge {
                max_bytes: self.max_retained_bytes,
                retained_bytes: wire_bytes,
            });
        }
        if bounded_callback_response_id(&response).is_none() {
            return Err(RpcCallbackResponseAdmissionError::InvalidResponseId);
        }
        if response.jsonrpc != crate::protocol::JSONRPC_VERSION {
            return Err(RpcCallbackResponseAdmissionError::UnsupportedVersion);
        }
        if response.result.is_some() == response.error.is_some() {
            return Err(RpcCallbackResponseAdmissionError::InvalidEnvelope);
        }

        // Callback consumers route only on the stable error code/message.
        // Drop attacker-controlled error data before the internal handoff so
        // it cannot bypass the retained-byte owner through `serde_json::Value`.
        if let Some(error) = response.error.as_mut() {
            if error.message.len() > RPC_CALLBACK_RESPONSE_MAX_ERROR_MESSAGE_BYTES {
                return Err(RpcCallbackResponseAdmissionError::ErrorMessageTooLarge {
                    max_bytes: RPC_CALLBACK_RESPONSE_MAX_ERROR_MESSAGE_BYTES,
                    actual_bytes: error.message.len(),
                });
            }
            error.data = None;
        }

        let retained_bytes = retained_rpc_callback_response_bytes(&response);
        if retained_bytes > self.max_retained_bytes {
            return Err(RpcCallbackResponseAdmissionError::ResponseTooLarge {
                max_bytes: self.max_retained_bytes,
                retained_bytes,
            });
        }
        let charged_bytes = retained_bytes
            .max(RPC_CALLBACK_RESPONSE_MIN_ADMISSION_BYTES)
            .saturating_mul(RPC_CALLBACK_RESPONSE_MEMORY_CHARGE_MULTIPLIER);
        if charged_bytes > self.capacity_bytes {
            return Err(RpcCallbackResponseAdmissionError::ResponseTooLarge {
                max_bytes: self
                    .capacity_bytes
                    .checked_div(RPC_CALLBACK_RESPONSE_MEMORY_CHARGE_MULTIPLIER)
                    .unwrap_or(0),
                retained_bytes,
            });
        }
        let permits = u32::try_from(charged_bytes)
            .map_err(|_| RpcCallbackResponseAdmissionError::PermitRange)?;
        let count_permit = match Arc::clone(&self.count_semaphore).try_acquire_owned() {
            Ok(permit) => permit,
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                return Err(RpcCallbackResponseAdmissionError::Backpressured {
                    max_inflight_bytes: self.capacity_bytes,
                    max_inflight: self.max_inflight,
                    requested_bytes: charged_bytes,
                });
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                return Err(RpcCallbackResponseAdmissionError::Closed);
            }
        };
        let memory_permit = match Arc::clone(&self.memory_semaphore).try_acquire_many_owned(permits)
        {
            Ok(permit) => permit,
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                return Err(RpcCallbackResponseAdmissionError::Backpressured {
                    max_inflight_bytes: self.capacity_bytes,
                    max_inflight: self.max_inflight,
                    requested_bytes: charged_bytes,
                });
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                return Err(RpcCallbackResponseAdmissionError::Closed);
            }
        };
        Ok(CallbackResponseHandoff::admitted(
            response,
            memory_permit,
            count_permit,
        ))
    }
}

fn retained_rpc_callback_response_bytes(response: &RpcResponse) -> usize {
    let id_bytes = match response.id.as_ref() {
        Some(RpcId::Str(id)) => id.len(),
        Some(RpcId::Num(_)) => std::mem::size_of::<i64>(),
        None => 0,
    };
    let result_bytes = response
        .result
        .as_deref()
        .map_or(0, |result| result.get().len());
    let error_message_bytes = response
        .error
        .as_ref()
        .map_or(0, |error| error.message.len());
    std::mem::size_of::<RpcResponse>()
        .saturating_add(response.jsonrpc.len())
        .saturating_add(id_bytes)
        .saturating_add(result_bytes)
        .saturating_add(error_message_bytes)
}

impl RpcRequestAdmission {
    fn production() -> Self {
        static PROCESS_ADMISSION: OnceLock<RpcRequestAdmission> = OnceLock::new();
        PROCESS_ADMISSION
            .get_or_init(|| {
                Self::new(
                    RPC_PROCESS_REQUEST_MEMORY_BUDGET_BYTES,
                    RPC_PROCESS_MAX_INFLIGHT_REQUESTS,
                )
            })
            .clone()
    }

    fn new(capacity_bytes: usize, max_inflight_requests: usize) -> Self {
        Self {
            memory_semaphore: Arc::new(Semaphore::new(capacity_bytes)),
            task_semaphore: Arc::new(Semaphore::new(max_inflight_requests)),
            capacity_bytes,
            max_inflight_requests,
        }
    }

    fn admit(
        &self,
        request: &RpcRequest,
    ) -> Result<RpcRequestAdmissionPermit, RpcRequestAdmissionError> {
        validate_rpc_request_envelope(request)?;
        let retained_bytes = retained_rpc_request_bytes(request)?;
        self.admit_retained(request, retained_bytes)
    }

    /// Bridge a parsed but invalid request out of frame-memory custody before
    /// writing its bounded rejection. This deliberately skips echo-envelope
    /// validation but still charges every retained byte and the task count.
    fn admit_unvalidated(
        &self,
        request: &RpcRequest,
    ) -> Result<RpcRequestAdmissionPermit, RpcRequestAdmissionError> {
        self.admit_retained(request, retained_rpc_request_bytes_unchecked(request))
    }

    fn admit_retained(
        &self,
        request: &RpcRequest,
        retained_bytes: usize,
    ) -> Result<RpcRequestAdmissionPermit, RpcRequestAdmissionError> {
        let charged_bytes = rpc_request_memory_charge_bytes(request, retained_bytes);
        if charged_bytes > self.capacity_bytes {
            return Err(RpcRequestAdmissionError::RequestTooLarge {
                max_bytes: self.capacity_bytes / RPC_REQUEST_MEMORY_CHARGE_MULTIPLIER,
                retained_bytes,
                charged_bytes,
            });
        }
        let permits = u32::try_from(charged_bytes)
            .map_err(|_| RpcRequestAdmissionError::PermitRange { charged_bytes })?;
        let task_permit = match Arc::clone(&self.task_semaphore).try_acquire_owned() {
            Ok(permit) => permit,
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                return Err(RpcRequestAdmissionError::Backpressured {
                    max_inflight_bytes: self.capacity_bytes,
                    max_inflight_requests: self.max_inflight_requests,
                    requested_bytes: charged_bytes,
                });
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                return Err(RpcRequestAdmissionError::Closed);
            }
        };
        let memory_permit = match Arc::clone(&self.memory_semaphore).try_acquire_many_owned(permits)
        {
            Ok(permit) => permit,
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                return Err(RpcRequestAdmissionError::Backpressured {
                    max_inflight_bytes: self.capacity_bytes,
                    max_inflight_requests: self.max_inflight_requests,
                    requested_bytes: charged_bytes,
                });
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                return Err(RpcRequestAdmissionError::Closed);
            }
        };
        Ok(RpcRequestAdmissionPermit {
            _memory_permit: memory_permit,
            _task_permit: task_permit,
        })
    }
}

fn live_send_input_memory_charge_bytes(retained_bytes: usize) -> usize {
    retained_bytes
        .max(LIVE_SEND_INPUT_MIN_ADMISSION_BYTES)
        .saturating_mul(RPC_REQUEST_MEMORY_CHARGE_MULTIPLIER)
}

fn live_open_memory_charge_bytes(retained_bytes: usize) -> usize {
    retained_bytes
        .max(LIVE_OPEN_MIN_ADMISSION_BYTES)
        .saturating_mul(RPC_REQUEST_MEMORY_CHARGE_MULTIPLIER)
}

fn rpc_request_memory_charge_bytes(request: &RpcRequest, retained_bytes: usize) -> usize {
    match request.method.as_str() {
        "live/send_input" => live_send_input_memory_charge_bytes(retained_bytes),
        "live/open" => live_open_memory_charge_bytes(retained_bytes),
        _ => retained_bytes
            .max(RPC_REQUEST_MIN_ADMISSION_BYTES)
            .saturating_mul(RPC_REQUEST_MEMORY_CHARGE_MULTIPLIER),
    }
}

#[derive(Clone)]
struct RpcTcpConnectionAdmission {
    semaphore: Arc<Semaphore>,
}

impl RpcTcpConnectionAdmission {
    fn production() -> Self {
        static PROCESS_ADMISSION: OnceLock<RpcTcpConnectionAdmission> = OnceLock::new();
        PROCESS_ADMISSION
            .get_or_init(|| Self::new(RPC_TCP_MAX_CONNECTIONS))
            .clone()
    }

    fn new(max_connections: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_connections)),
        }
    }

    fn try_acquire(&self) -> Result<OwnedSemaphorePermit, ServerError> {
        Arc::clone(&self.semaphore)
            .try_acquire_owned()
            .map_err(|_| {
                ServerError::Io(std::io::Error::other(
                    "RPC TCP connection admission is saturated or closed",
                ))
            })
    }
}

fn retained_rpc_request_bytes(request: &RpcRequest) -> Result<usize, RpcRequestAdmissionError> {
    let id_bytes = rpc_request_id_bytes(request)?;
    Ok(retained_rpc_request_bytes_with_id(request, id_bytes))
}

fn retained_rpc_request_bytes_unchecked(request: &RpcRequest) -> usize {
    let id_bytes = match request.id.as_ref() {
        Some(RpcId::Str(id)) => id.len(),
        Some(RpcId::Num(_)) => std::mem::size_of::<i64>(),
        None => 0,
    };
    retained_rpc_request_bytes_with_id(request, id_bytes)
}

fn retained_rpc_request_bytes_with_id(request: &RpcRequest, id_bytes: usize) -> usize {
    let params_bytes = request
        .params
        .as_deref()
        .map_or(0, |params| params.get().len());
    std::mem::size_of::<RpcRequest>()
        .saturating_add(request.jsonrpc.len())
        .saturating_add(request.method.len())
        .saturating_add(id_bytes)
        .saturating_add(params_bytes)
}

fn rpc_request_id_bytes(request: &RpcRequest) -> Result<usize, RpcRequestAdmissionError> {
    match request.id.as_ref() {
        Some(RpcId::Str(id)) => {
            if id.len() > RPC_MAX_STRING_ID_BYTES {
                return Err(RpcRequestAdmissionError::RequestIdTooLarge {
                    max_bytes: RPC_MAX_STRING_ID_BYTES,
                    actual_bytes: id.len(),
                });
            }
            Ok(id.len())
        }
        Some(RpcId::Num(_)) => Ok(std::mem::size_of::<i64>()),
        None => Ok(0),
    }
}

fn bounded_callback_response_id(response: &RpcResponse) -> Option<RpcId> {
    match response.id.as_ref()? {
        RpcId::Str(id) if id.len() > RPC_MAX_STRING_ID_BYTES => None,
        id => Some(id.clone()),
    }
}

fn validate_rpc_request_envelope(request: &RpcRequest) -> Result<(), RpcRequestAdmissionError> {
    let id_bytes = rpc_request_id_bytes(request)?;
    if request.method.len() > RPC_MAX_METHOD_BYTES {
        return Err(RpcRequestAdmissionError::RequestMethodTooLarge {
            max_bytes: RPC_MAX_METHOD_BYTES,
            actual_bytes: request.method.len(),
        });
    }
    let envelope_bytes = std::mem::size_of::<RpcRequest>()
        .saturating_add(request.jsonrpc.len())
        .saturating_add(request.method.len())
        .saturating_add(id_bytes);
    if envelope_bytes > RPC_MAX_REQUEST_ENVELOPE_BYTES {
        return Err(RpcRequestAdmissionError::RequestEnvelopeTooLarge {
            max_bytes: RPC_MAX_REQUEST_ENVELOPE_BYTES,
            actual_bytes: envelope_bytes,
        });
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum RpcRequestAdmissionError {
    #[error(
        "RPC request is too large for admission: retained={retained_bytes} bytes, charged={charged_bytes} bytes (maximum {max_bytes})"
    )]
    RequestTooLarge {
        max_bytes: usize,
        retained_bytes: usize,
        charged_bytes: usize,
    },
    #[error("JSON-RPC string id is too large: {actual_bytes} bytes (maximum {max_bytes})")]
    RequestIdTooLarge {
        max_bytes: usize,
        actual_bytes: usize,
    },
    #[error("JSON-RPC method is too large: {actual_bytes} bytes (maximum {max_bytes})")]
    RequestMethodTooLarge {
        max_bytes: usize,
        actual_bytes: usize,
    },
    #[error("JSON-RPC request envelope is too large: {actual_bytes} bytes (maximum {max_bytes})")]
    RequestEnvelopeTooLarge {
        max_bytes: usize,
        actual_bytes: usize,
    },
    #[error(
        "RPC admission is backpressured: requested {requested_bytes} bytes from a {max_inflight_bytes}-byte / {max_inflight_requests}-request inflight budget"
    )]
    Backpressured {
        max_inflight_bytes: usize,
        max_inflight_requests: usize,
        requested_bytes: usize,
    },
    #[error("RPC request charge {charged_bytes} bytes exceeds the admission permit range")]
    PermitRange { charged_bytes: usize },
    #[error("RPC request admission is closed")]
    Closed,
}

impl RpcRequestAdmissionError {
    fn reason_code(&self) -> &'static str {
        match self {
            Self::RequestTooLarge { .. } => "rpc_request_too_large",
            Self::RequestIdTooLarge { .. } => "rpc_request_id_too_large",
            Self::RequestMethodTooLarge { .. } => "rpc_request_method_too_large",
            Self::RequestEnvelopeTooLarge { .. } => "rpc_request_envelope_too_large",
            Self::Backpressured { .. } => "rpc_request_backpressured",
            Self::PermitRange { .. } => "rpc_request_permit_range",
            Self::Closed => "rpc_request_admission_closed",
        }
    }

    fn jsonrpc_code(&self) -> i32 {
        match self {
            Self::RequestIdTooLarge { .. }
            | Self::RequestMethodTooLarge { .. }
            | Self::RequestEnvelopeTooLarge { .. } => crate::error::INVALID_REQUEST,
            Self::RequestTooLarge { .. } | Self::PermitRange { .. } => crate::error::INVALID_PARAMS,
            Self::Backpressured { .. } => crate::error::BUDGET_EXHAUSTED,
            Self::Closed => crate::error::INTERNAL_ERROR,
        }
    }

    fn response_id(&self, request: &RpcRequest) -> Option<RpcId> {
        if matches!(
            self,
            Self::RequestIdTooLarge { .. }
                | Self::RequestMethodTooLarge { .. }
                | Self::RequestEnvelopeTooLarge { .. }
        ) {
            None
        } else {
            request.id.clone()
        }
    }

    fn response_data(&self) -> serde_json::Value {
        match self {
            Self::RequestTooLarge {
                max_bytes,
                retained_bytes,
                ..
            } => serde_json::json!({
                "reason": self.reason_code(),
                "max_bytes": max_bytes,
                "retained_bytes": retained_bytes,
            }),
            Self::RequestIdTooLarge {
                max_bytes,
                actual_bytes,
            } => serde_json::json!({
                "reason": self.reason_code(),
                "max_bytes": max_bytes,
                "actual_bytes": actual_bytes,
            }),
            Self::RequestMethodTooLarge {
                max_bytes,
                actual_bytes,
            }
            | Self::RequestEnvelopeTooLarge {
                max_bytes,
                actual_bytes,
            } => serde_json::json!({
                "reason": self.reason_code(),
                "max_bytes": max_bytes,
                "actual_bytes": actual_bytes,
            }),
            Self::Backpressured {
                max_inflight_bytes,
                max_inflight_requests,
                ..
            } => serde_json::json!({
                "reason": self.reason_code(),
                "max_inflight_bytes": max_inflight_bytes,
                "max_inflight_requests": max_inflight_requests,
            }),
            Self::PermitRange { charged_bytes } => serde_json::json!({
                "reason": self.reason_code(),
                "charged_bytes": charged_bytes,
            }),
            Self::Closed => serde_json::json!({
                "reason": self.reason_code(),
            }),
        }
    }
}

fn rpc_admission_error_response(
    request: &RpcRequest,
    error: &RpcRequestAdmissionError,
) -> RpcResponse {
    let data = if request.method == "live/send_input" {
        match error {
            RpcRequestAdmissionError::RequestTooLarge {
                max_bytes,
                retained_bytes,
                ..
            } => serde_json::to_value(LiveSendInputErrorData {
                error_code: WireLiveAdapterErrorCode::ConfigRejected {
                    reason: WireLiveConfigRejectionReason::InputTooLarge {
                        max_bytes: u64::try_from(*max_bytes).unwrap_or(u64::MAX),
                        actual_bytes: u64::try_from(*retained_bytes).unwrap_or(u64::MAX),
                    },
                },
            })
            .unwrap_or_else(|_| serde_json::json!({"error_code":{"code":"internal_error"}})),
            RpcRequestAdmissionError::Backpressured {
                max_inflight_bytes, ..
            } => serde_json::to_value(LiveSendInputErrorData {
                error_code: WireLiveAdapterErrorCode::ConfigRejected {
                    reason: WireLiveConfigRejectionReason::InputBackpressured {
                        max_pending_bytes: u64::try_from(*max_inflight_bytes).unwrap_or(u64::MAX),
                    },
                },
            })
            .unwrap_or_else(|_| serde_json::json!({"error_code":{"code":"internal_error"}})),
            _ => error.response_data(),
        }
    } else {
        error.response_data()
    };
    RpcResponse::error_with_data(
        error.response_id(request),
        error.jsonrpc_code(),
        error.to_string(),
        data,
    )
}

struct LongRunningResponse {
    request_key: String,
    success: bool,
    response: RpcResponse,
    _request_permit: RpcRequestAdmissionPermit,
}

/// Ordinary response plus any request-memory reservation that must remain
/// held until serialization and socket write complete. Releasing at channel
/// handoff lets a non-reading client turn bounded request memory into
/// unbounded queued response memory.
struct AdmittedRpcResponse {
    response: RpcResponse,
    _request_permit: RpcRequestAdmissionPermit,
}

/// Errors from the RPC server.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// Transport-level error.
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// JSON-RPC server over async reader/writer streams.
pub struct RpcServer<R, W> {
    transport: JsonlTransport<R, W>,
    router: MethodRouter,
    notification_rx: mpsc::Receiver<RpcNotification>,
    /// Channel for responses from concurrently dispatched requests.
    response_rx: mpsc::Receiver<AdmittedRpcResponse>,
    response_tx: mpsc::Sender<AdmittedRpcResponse>,
    /// Channel for callback requests sent from tool dispatchers → server loop → client.
    callback_request_rx: mpsc::Receiver<CallbackRequestEnvelope>,
    /// Sender half cloned into `CallbackToolDispatcher` instances.
    callback_request_tx: mpsc::Sender<CallbackRequestEnvelope>,
    /// Outstanding server→client requests awaiting a response.
    pending_callbacks: HashMap<RpcId, oneshot::Sender<CallbackResponseHandoff>>,
    /// Counter for generating server-originated request IDs.
    callback_id_counter: Arc<AtomicU64>,
    long_running_tx: mpsc::Sender<LongRunningResponse>,
    long_running_rx: mpsc::Receiver<LongRunningResponse>,
    request_executor: SurfaceRequestExecutor,
    /// Process-wide count/byte admission held from before any request task
    /// spawn through response serialization/transport write (or notification
    /// completion). Every stdio/embedded/TCP server constructor shares this
    /// production owner; tests may inject an isolated budget.
    rpc_request_admission: RpcRequestAdmission,
    /// Reserved process-wide cancellation owner, independent of data-plane
    /// request saturation.
    rpc_cancel_admission: RpcCancelAdmission,
    /// Process-wide active full-frame read owner. Connections wait for a
    /// buffered byte before acquiring it, so idle sockets retain only their
    /// small `BufReader`/task state rather than maximum-frame capacity.
    rpc_frame_read_admission: JsonlFrameAdmission,
    /// Separate process-wide response admission for client callback payloads.
    /// Its permits cross the oneshot boundary and remain held while the tool
    /// dispatcher parses the retained raw result into core-owned content.
    callback_response_admission: RpcCallbackResponseAdmission,
    /// When true, skip runtime shutdown on EOF. Used by `serve_tcp` where the
    /// runtime is shared across sequential connections and must not be destroyed
    /// when a single client disconnects.
    pub skip_shutdown_on_eof: bool,
}

impl<R: AsyncBufRead + Unpin, W: TransportWriter> RpcServer<R, W> {
    /// Create a new RPC server.
    ///
    /// The server reads JSON-RPC requests from `reader`, dispatches them
    /// via the `MethodRouter`, and writes responses/notifications to `writer`.
    pub fn new(
        reader: R,
        writer: W,
        runtime: Arc<SessionRuntime>,
        config_store: Arc<dyn ConfigStore>,
    ) -> Self {
        Self::new_with_skill_runtime(reader, writer, runtime, config_store, None)
    }

    /// Create a new RPC server with an optional skill runtime for introspection.
    pub fn new_with_skill_runtime(
        reader: R,
        writer: W,
        runtime: Arc<SessionRuntime>,
        config_store: Arc<dyn ConfigStore>,
        skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    ) -> Self {
        let (notification_tx, notification_rx) = mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);
        let notification_sink = NotificationSink::new(notification_tx);
        let transport = JsonlTransport::new(reader, writer);
        let (response_tx, response_rx) = mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);
        let (callback_request_tx, callback_request_rx) =
            mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);
        let callback_id_counter = Arc::new(AtomicU64::new(0));
        let registered_tools = Arc::new(std::sync::RwLock::new(Vec::new()));
        let (long_running_tx, long_running_rx) = mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);

        // Wire callback tool state into the runtime so session/create handlers
        // can build CallbackToolDispatchers that route through this server.
        runtime.set_callback_channel(
            callback_request_tx.clone(),
            callback_id_counter.clone(),
            registered_tools,
        );

        let router = MethodRouter::new(runtime, config_store, notification_sink)
            .with_skill_runtime(skill_runtime);
        Self {
            transport,
            router,
            notification_rx,
            response_rx,
            response_tx,
            callback_request_rx,
            callback_request_tx,
            pending_callbacks: HashMap::new(),
            callback_id_counter,
            long_running_tx,
            long_running_rx,
            request_executor: SurfaceRequestExecutor::new(tokio::time::Duration::from_secs(5)),
            rpc_request_admission: RpcRequestAdmission::production(),
            rpc_cancel_admission: RpcCancelAdmission::production(),
            rpc_frame_read_admission: JsonlFrameAdmission::production(),
            callback_response_admission: RpcCallbackResponseAdmission::production(),
            skip_shutdown_on_eof: false,
        }
    }

    pub fn with_live_session_factory_opt(
        mut self,
        factory: Option<Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory>>,
    ) -> Self {
        if let Some(factory) = factory {
            self.router = self.router.with_live_session_factory(factory);
        }
        self
    }

    fn with_rpc_request_admission(mut self, admission: RpcRequestAdmission) -> Self {
        self.rpc_request_admission = admission;
        self
    }

    #[cfg(test)]
    fn with_rpc_frame_read_admission(mut self, admission: JsonlFrameAdmission) -> Self {
        self.rpc_frame_read_admission = admission;
        self
    }

    #[cfg(test)]
    fn with_callback_response_admission(mut self, admission: RpcCallbackResponseAdmission) -> Self {
        self.callback_response_admission = admission;
        self
    }

    /// Attach a live WebSocket transport for `live/open` token minting.
    pub fn with_live_ws(
        mut self,
        state: std::sync::Arc<meerkat_live::LiveWsState>,
        base_url: String,
    ) -> Self {
        self.router = self.router.with_live_ws(state, base_url);
        self
    }

    /// Attach a live WebRTC transport for `live/open` token minting and SDP
    /// answer signaling.
    #[cfg(feature = "live-webrtc")]
    pub fn with_live_webrtc(
        mut self,
        state: std::sync::Arc<meerkat_live::LiveWebrtcState>,
    ) -> Self {
        self.router = self.router.with_live_webrtc(state);
        self
    }

    #[cfg(feature = "mob")]
    /// Create a new RPC server with an optional skill runtime and explicit mob state.
    ///
    /// `callback_rx` is the receiver half of the callback channel, pre-created via
    /// `SessionRuntime::init_callback_channel()`. This ensures the callback channel
    /// exists before mob resume (so `ExternalToolsProvider` closures can read it),
    /// while the server owns the receiver for the `tool/execute` dispatch loop.
    pub fn new_with_skill_runtime_and_mob_state(
        reader: R,
        writer: W,
        runtime: Arc<SessionRuntime>,
        config_store: Arc<dyn ConfigStore>,
        skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
        mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
        callback_request_rx: mpsc::Receiver<CallbackRequestEnvelope>,
    ) -> Self {
        let (notification_tx, notification_rx) = mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);
        let notification_sink = NotificationSink::new(notification_tx);
        let transport = JsonlTransport::new(reader, writer);
        let (response_tx, response_rx) = mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);
        let (long_running_tx, long_running_rx) = mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);

        // Callback channel was pre-created by the caller via init_callback_channel().
        // Read the shared state (tx, counter, tools) from the runtime.
        // If somehow not initialized, the callback_request_rx the caller passed
        // will never receive, but the server won't crash.
        let callback_request_tx = runtime.callback_request_tx().unwrap_or_else(|| {
            tracing::warn!(
                "callback channel not pre-initialized on runtime; callback tools will not work"
            );
            let (tx, _) = mpsc::channel(1);
            tx
        });
        let callback_id_counter = runtime.callback_id_counter();

        let router =
            MethodRouter::new_with_mob_state(runtime, config_store, notification_sink, mob_state)
                .with_skill_runtime(skill_runtime);
        Self {
            transport,
            router,
            notification_rx,
            response_rx,
            response_tx,
            callback_request_rx,
            callback_request_tx,
            pending_callbacks: HashMap::new(),
            callback_id_counter,
            long_running_tx,
            long_running_rx,
            request_executor: SurfaceRequestExecutor::new(tokio::time::Duration::from_secs(5)),
            rpc_request_admission: RpcRequestAdmission::production(),
            rpc_cancel_admission: RpcCancelAdmission::production(),
            rpc_frame_read_admission: JsonlFrameAdmission::production(),
            callback_response_admission: RpcCallbackResponseAdmission::production(),
            skip_shutdown_on_eof: false,
        }
    }

    /// Get the callback request sender for creating `CallbackToolDispatcher` instances.
    pub fn callback_request_tx(&self) -> mpsc::Sender<CallbackRequestEnvelope> {
        self.callback_request_tx.clone()
    }

    /// Get the callback ID counter for generating unique server-originated IDs.
    pub fn callback_id_counter(&self) -> Arc<AtomicU64> {
        self.callback_id_counter.clone()
    }

    /// Get the shared registered tools list.
    pub fn registered_tools(&self) -> Arc<std::sync::RwLock<Vec<meerkat_core::ToolDef>>> {
        self.router.runtime().registered_tools()
    }

    /// Run the server until EOF or a fatal transport error.
    ///
    /// Requests are dispatched concurrently in spawned tasks so that long-running
    /// operations (e.g. `session/create`, `turn/start`) do not block the server
    /// from processing other requests (e.g. `turn/interrupt`).
    ///
    /// Parse errors are reported to the client and do not terminate the loop.
    /// EOF (reader returns `None`) triggers graceful shutdown.
    pub async fn run(&mut self) -> Result<(), ServerError> {
        // Periodic sweep for timed-out callback entries. Fires even when
        // the connection is idle, preventing stale leaks.
        let mut callback_sweep = tokio::time::interval(tokio::time::Duration::from_secs(30));
        callback_sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            let frame_read_admission = self.rpc_frame_read_admission.clone();
            tokio::select! {
                // Read the next message from the transport.
                msg = self.transport.read_message_admitted(&frame_read_admission) => {
                    match msg {
                        Ok(Some(AdmittedJsonlMessage {
                            message: RpcMessage::Request(request),
                            wire_bytes: _,
                            _frame_memory_permit,
                        })) => {
                            // Cancellation has a dedicated reserved owner. Acquire
                            // it while frame memory still owns the parsed request,
                            // then release frame memory before validation, target
                            // parsing, or executor I/O.
                            if request.is_notification() && request.method == "cancel" {
                                let Some(cancel_permit) = self.rpc_cancel_admission.admit(&request)
                                else {
                                    drop(_frame_memory_permit);
                                    continue;
                                };
                                drop(_frame_memory_permit);
                                if validate_rpc_request_envelope(&request).is_ok()
                                    && request.has_supported_version()
                                    && let Some(target) =
                                        request_cancel_target(request.params.as_deref())
                                {
                                    let _ = self.request_executor.cancel_request(&target).await;
                                }
                                drop(cancel_permit);
                                continue;
                            }

                            // Validate the echo-bearing request id before any
                            // other rejection path can clone it into a response.
                            // This applies to every RPC method; live-input
                            // admission reuses the same validated size when it
                            // charges the full retained request shape.
                            if let Err(error) = validate_rpc_request_envelope(&request) {
                                // JSON-RPC notifications never receive a response,
                                // including when their bounded envelope is invalid.
                                // The parse-error path above remains distinct because
                                // no request shape could be recovered there.
                                if request.is_notification() {
                                    drop(_frame_memory_permit);
                                } else {
                                    let response = rpc_admission_error_response(&request, &error);
                                    drop(request);
                                    drop(_frame_memory_permit);
                                    self.transport.write_response(&response).await?;
                                }
                                continue;
                            }

                            // Bridge parsed request custody from incremental
                            // frame memory into process-shared request count and
                            // bytes before any dispatch or socket I/O.
                            let request_permit = match self.rpc_request_admission.admit(&request) {
                                Ok(permit) => permit,
                                Err(error) => {
                                    if request.is_notification() {
                                        drop(_frame_memory_permit);
                                    } else {
                                        let response =
                                            rpc_admission_error_response(&request, &error);
                                        drop(request);
                                        drop(_frame_memory_permit);
                                        self.transport.write_response(&response).await?;
                                    }
                                    continue;
                                }
                            };
                            drop(_frame_memory_permit);

                            // Validate the JSON-RPC envelope version at the
                            // transport boundary before any method-specific
                            // routing. Frames that do not declare the supported
                            // "2.0" version are rejected with the standard
                            // INVALID_REQUEST error (or dropped, if a
                            // notification) rather than dispatched.
                            if !request.has_supported_version() {
                                if !request.is_notification() {
                                    let response = RpcResponse::error(
                                        request.id.clone(),
                                        crate::error::INVALID_REQUEST,
                                        format!(
                                            "Unsupported JSON-RPC version: expected \"{}\", got \"{}\"",
                                            crate::protocol::JSONRPC_VERSION,
                                            request.jsonrpc
                                        ),
                                    );
                                    self.transport.write_response(&response).await?;
                                }
                                drop(request_permit);
                                continue;
                            }

                            if requires_connection_ordered_dispatch(&request) {
                                // Callback tool registration must commit before later
                                // pipelined frames on this connection can observe tools.
                                if let Some(response) = self.router.dispatch(request).await {
                                    self.transport.write_response(&response).await?;
                                }
                                drop(request_permit);
                                continue;
                            }

                            let long_running_id = request.id.clone().filter(|_| {
                                request_semantics(&request).requires_long_running_executor()
                            });
                            if let Some(request_id) = long_running_id {
                                match self.spawn_long_running_request(
                                    request,
                                    request_id,
                                    request_permit,
                                ) {
                                    Ok((request_key, task)) => {
                                        self.request_executor
                                            .attach_task(&request_key, task);
                                    }
                                    Err(error) => {
                                        let (response, request_permit) = *error;
                                        self.transport.write_response(&response).await?;
                                        drop(request_permit);
                                    }
                                }
                                continue;
                            }

                            let router = self.router.clone();
                            let resp_tx = self.response_tx.clone();
                            tokio::spawn(async move {
                                let response = Box::pin(router.dispatch(request)).await;
                                // Admission is non-blocking in the server loop.
                                // Move the reservation with the response so it
                                // remains held across queueing, serialization,
                                // and the potentially blocking socket write.
                                if let Some(response) = response {
                                    let _ = resp_tx
                                        .send(AdmittedRpcResponse {
                                            response,
                                            _request_permit: request_permit,
                                        })
                                        .await;
                                }
                            });
                        }
                        Ok(Some(AdmittedJsonlMessage {
                            message: RpcMessage::Response(response),
                            wire_bytes,
                            _frame_memory_permit,
                        })) => {
                            // Callback response from the client.
                            let Some(response_id) = bounded_callback_response_id(&response) else {
                                tracing::warn!(
                                    "dropping callback response with missing or oversized id"
                                );
                                continue;
                            };
                            if let Some(tx) = self.pending_callbacks.remove(&response_id) {
                                let handoff = match self
                                    .callback_response_admission
                                    .admit(response, wire_bytes)
                                {
                                    Ok(handoff) => handoff,
                                    Err(error) => {
                                        tracing::warn!(
                                            id = ?response_id,
                                            error = %error,
                                            "rejecting callback response before internal handoff"
                                        );
                                        CallbackResponseHandoff::rejected(RpcResponse::error(
                                            Some(response_id.clone()),
                                            error.jsonrpc_code(),
                                            error.to_string(),
                                        ))
                                    }
                                };
                                drop(_frame_memory_permit);
                                let _ = tx.send(handoff);
                            } else {
                                drop(_frame_memory_permit);
                                tracing::warn!(
                                    id = ?response_id,
                                    "Received response for unknown callback ID"
                                );
                            }
                        }
                        Ok(None) => break, // EOF - clean shutdown
                        Err(TransportError::Parse(err)) => {
                            let response = RpcResponse::error(
                                None,
                                crate::error::PARSE_ERROR,
                                format!("Parse error: {err}"),
                            );
                            self.transport.write_response(&response).await?;
                        }
                        Err(error @ (TransportError::FrameTooLarge { .. }
                            | TransportError::FrameAdmissionBackpressured { .. }
                            | TransportError::FrameProgressTimeout)) => {
                            // The bounded reader stops as soon as the limit is
                            // crossed, leaving the connection mid-frame. Treat
                            // that protocol violation as fatal rather than
                            // interpreting the remainder as another message.
                            return Err(ServerError::Transport(error));
                        }
                        Err(TransportError::Io(err)) => {
                            return Err(ServerError::Io(err));
                        }
                        Err(TransportError::WriteTimeout) => {
                            return Err(ServerError::Transport(TransportError::WriteTimeout));
                        }
                    }
                }

                // Write responses from completed dispatch tasks.
                Some(admitted) = self.response_rx.recv() => {
                    self.transport.write_response(&admitted.response).await?;
                    drop(admitted);
                }

                Some(response) = self.long_running_rx.recv() => {
                    if !self.write_long_running_response(response).await {
                        break;
                    }
                }

                // Forward queued notifications to the transport.
                Some(notification) = self.notification_rx.recv() => {
                    self.transport.write_notification(&notification).await?;
                }

                // Send callback requests to the client.
                Some(envelope) = self.callback_request_rx.recv() => {
                    let (request, response_tx, _outbound_permit) = envelope.into_parts();
                    if let Some(ref req_id) = request.id {
                        self.pending_callbacks.insert(req_id.clone(), response_tx);
                    }
                    self.transport.write_request(&request).await?;
                    drop(_outbound_permit);
                }

                // Periodic sweep of timed-out callback entries.
                _ = callback_sweep.tick() => {
                    self.pending_callbacks.retain(|_, tx| !tx.is_closed());
                }
            }
        }

        // Graceful shutdown: close all sessions (unless this is a shared TCP
        // runtime where client disconnect should not destroy state).
        self.request_executor.shutdown_and_abort_stragglers().await;
        if !self.skip_shutdown_on_eof {
            self.router.runtime().shutdown().await;
        }
        Ok(())
    }

    fn spawn_long_running_request(
        &self,
        request: RpcRequest,
        id: RpcId,
        request_permit: RpcRequestAdmissionPermit,
    ) -> Result<(String, tokio::task::JoinHandle<()>), Box<(RpcResponse, RpcRequestAdmissionPermit)>>
    {
        let semantics = request_semantics(&request);
        debug_assert!(semantics.requires_long_running_executor());
        let request_key = request_key(&id);
        let context = match self.request_executor.try_begin_request_with_semantics(
            request_key.clone(),
            noop_request_action(),
            semantics,
        ) {
            Ok(context) => context,
            Err(error) => {
                return Err(Box::new((
                    request_admission_error_response(Some(id), error),
                    request_permit,
                )));
            }
        };
        let router = self.router.clone();
        let long_running_tx = self.long_running_tx.clone();
        let request_key_for_task = request_key.clone();
        let handle = tokio::spawn(async move {
            if let Some(response) =
                Box::pin(router.dispatch_with_request_context(request, Some(context))).await
            {
                let success = response.error.is_none();
                let _ = long_running_tx
                    .send(LongRunningResponse {
                        request_key: request_key_for_task,
                        success,
                        response,
                        _request_permit: request_permit,
                    })
                    .await;
            }
        });
        Ok((request_key, handle))
    }

    async fn write_long_running_response(&mut self, response: LongRunningResponse) -> bool {
        let cancel_id = response.response.id.clone();
        let to_write = match self
            .request_executor
            .resolve_admitted_terminal(
                Some(&response.request_key),
                response.success,
                response.response,
            )
            .await
        {
            RequestTerminalResolution::Emit(rpc_response) => rpc_response,
            RequestTerminalResolution::Cancelled => request_cancelled_response(cancel_id),
            RequestTerminalResolution::LifecycleError(err) => {
                tracing::warn!(
                    request_key = %response.request_key,
                    error = %err,
                    "request lifecycle rejected publish response"
                );
                request_lifecycle_error_response(cancel_id, err)
            }
        };

        self.transport.write_response(&to_write).await.is_ok()
    }
}

fn request_key(id: &RpcId) -> String {
    serde_json::to_string(id).unwrap_or_else(|_| format!("{id:?}"))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestCancelParams {
    request_id: RpcId,
}

fn request_cancel_target(params: Option<&serde_json::value::RawValue>) -> Option<String> {
    let raw = params?;
    if raw.get().len() > RPC_MAX_CANCEL_PARAMS_BYTES {
        return None;
    }
    let params: RequestCancelParams = serde_json::from_str(raw.get()).ok()?;
    if matches!(
        &params.request_id,
        RpcId::Str(id) if id.len() > RPC_MAX_STRING_ID_BYTES
    ) {
        return None;
    }
    Some(request_key(&params.request_id))
}

fn request_cancelled_response(id: Option<RpcId>) -> RpcResponse {
    RpcResponse::error(
        id,
        crate::error::REQUEST_CANCELLED,
        "request cancelled before response publish",
    )
}

fn request_lifecycle_error_response(
    id: Option<RpcId>,
    err: meerkat::surface::RequestTransitionError,
) -> RpcResponse {
    RpcResponse::error(
        id,
        crate::error::INTERNAL_ERROR,
        format!("request lifecycle rejected publish response: {err}"),
    )
}

fn request_admission_error_response(id: Option<RpcId>, err: RequestAdmissionError) -> RpcResponse {
    match err {
        RequestAdmissionError::AlreadyExists => RpcResponse::error(
            id,
            crate::error::DUPLICATE_INPUT,
            "request already admitted",
        ),
        RequestAdmissionError::AuthorityRejected { .. } => RpcResponse::error(
            id,
            crate::error::INTERNAL_ERROR,
            format!("request admission rejected by generated authority: {err}"),
        ),
    }
}

fn request_semantics(request: &RpcRequest) -> SurfaceRequestSemantics {
    SurfaceRequestSemantics::from(rpc_request_lifecycle(
        request.method.as_str(),
        request.params.as_deref().map(|params| params.get()),
    ))
}

fn requires_connection_ordered_dispatch(request: &RpcRequest) -> bool {
    request.method == "tools/register"
}

/// Start the RPC server on stdin/stdout.
///
/// This is the main entry point called from the CLI `rpc` subcommand.
pub async fn serve_stdio(
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
) -> Result<(), ServerError> {
    serve_stdio_with_skill_runtime(runtime, config_store, None).await
}

/// Start the RPC server on stdin/stdout with an optional skill runtime.
pub async fn serve_stdio_with_skill_runtime(
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
) -> Result<(), ServerError> {
    serve_stdio_with_options(runtime, config_store, skill_runtime, None).await
}

/// Start the RPC server on stdin/stdout with optional live WebSocket config.
pub async fn serve_stdio_with_options(
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    live: Option<LiveConfig>,
) -> Result<(), ServerError> {
    let stdin = tokio::io::stdin();
    let stdout = BlockingWriter::stdout();
    let reader = BufReader::new(stdin);
    let mut server =
        RpcServer::new_with_skill_runtime(reader, stdout, runtime, config_store, skill_runtime);
    if let Some(live) = live {
        if let Some(ws) = live.ws {
            server = server.with_live_ws(ws.state, ws.base_url);
        }
        #[cfg(feature = "live-webrtc")]
        if let Some(webrtc_state) = live.webrtc_state {
            server = server.with_live_webrtc(webrtc_state);
        }
        server = server.with_live_session_factory_opt(live.session_factory);
    }
    server.run().await
}

/// Optional live transport configuration to thread into the RPC server.
#[derive(Clone)]
pub struct LiveConfig {
    pub ws: Option<LiveWsConfig>,
    #[cfg(feature = "live-webrtc")]
    pub webrtc_state: Option<Arc<meerkat_live::LiveWebrtcState>>,
    pub session_factory: Option<Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory>>,
}

/// Optional live WebSocket configuration to thread into the RPC server.
#[derive(Clone)]
pub struct LiveWsConfig {
    pub state: Arc<meerkat_live::LiveWsState>,
    pub base_url: String,
}

/// Accept a single RPC client over an already-connected TCP stream.
///
/// The stream is split into read/write halves and wired directly into the
/// generic `RpcServer`. The caller is responsible for the accept loop —
/// this function handles exactly one connection lifetime.
pub async fn serve_tcp_connection(
    stream: tokio::net::TcpStream,
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
) -> Result<(), ServerError> {
    serve_tcp_connection_with_options(stream, runtime, config_store, skill_runtime, None).await
}

pub async fn serve_tcp_connection_with_options(
    stream: tokio::net::TcpStream,
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    live: Option<LiveConfig>,
) -> Result<(), ServerError> {
    let connection_permit = RpcTcpConnectionAdmission::production().try_acquire()?;
    serve_tcp_connection_with_options_and_admission(
        stream,
        runtime,
        config_store,
        skill_runtime,
        live,
        None,
        connection_permit,
    )
    .await
}

async fn serve_tcp_connection_with_options_and_admission(
    stream: tokio::net::TcpStream,
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    live: Option<LiveConfig>,
    admission: Option<RpcRequestAdmission>,
    _connection_permit: OwnedSemaphorePermit,
) -> Result<(), ServerError> {
    let (read_half, write_half) = stream.into_split();
    let reader = BufReader::new(read_half);
    let mut server =
        RpcServer::new_with_skill_runtime(reader, write_half, runtime, config_store, skill_runtime);
    if let Some(admission) = admission {
        server = server.with_rpc_request_admission(admission);
    }
    if let Some(live) = live {
        if let Some(ws) = live.ws {
            server = server.with_live_ws(ws.state, ws.base_url);
        }
        #[cfg(feature = "live-webrtc")]
        if let Some(webrtc_state) = live.webrtc_state {
            server = server.with_live_webrtc(webrtc_state);
        }
        server = server.with_live_session_factory_opt(live.session_factory);
    }
    server.skip_shutdown_on_eof = true;
    server.run().await
}

pub async fn serve_tcp(
    addr: &str,
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
) -> Result<(), ServerError> {
    serve_tcp_with_options(addr, runtime, config_store, skill_runtime, None).await
}

pub async fn serve_tcp_with_options(
    addr: &str,
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    live: Option<LiveConfig>,
) -> Result<(), ServerError> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // Clone the true process owner shared by every RPC surface and listener.
    // Per-listener semaphores would let an attacker multiply the ceiling by
    // opening sockets against independently mounted endpoints.
    let rpc_request_admission = RpcRequestAdmission::production();
    let connection_admission = RpcTcpConnectionAdmission::production();
    tracing::info!("RPC TCP listener bound to {addr}");
    loop {
        let (stream, peer_addr) = listener.accept().await?;
        // Accept first, then fail closed without parking or spawning when the
        // process-wide socket/task ceiling is saturated. Pre-acquiring here
        // would stop draining the listener and turn one idle socket into a
        // backlog-level denial of service.
        let connection_permit = match connection_admission.try_acquire() {
            Ok(permit) => permit,
            Err(error) => {
                tracing::warn!(%peer_addr, %error, "rejecting RPC TCP connection");
                drop(stream);
                continue;
            }
        };
        tracing::info!("RPC client connected from {peer_addr}");
        let runtime = Arc::clone(&runtime);
        let config_store = Arc::clone(&config_store);
        let skill_runtime = skill_runtime.clone();
        let live = live.clone();
        let admission = rpc_request_admission.clone();
        tokio::spawn(async move {
            if let Err(e) = serve_tcp_connection_with_options_and_admission(
                stream,
                runtime,
                config_store,
                skill_runtime,
                live,
                Some(admission),
                connection_permit,
            )
            .await
            {
                tracing::warn!("RPC client {peer_addr} disconnected: {e}");
            } else {
                tracing::info!("RPC client {peer_addr} disconnected cleanly");
            }
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn raw_params(value: serde_json::Value) -> Box<serde_json::value::RawValue> {
        let raw = serde_json::value::to_raw_value(&value);
        assert!(raw.is_ok(), "raw params should serialize: {raw:?}");
        match raw {
            Ok(raw) => raw,
            Err(_) => unreachable!("assert above"),
        }
    }

    #[test]
    fn deferred_session_create_uses_simple_path() {
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: "session/create".to_string(),
            params: Some(raw_params(serde_json::json!({
                "prompt": "hello",
                "initial_turn": "deferred"
            }))),
        };

        assert!(!request_semantics(&request).requires_long_running_executor());
    }

    #[test]
    fn immediate_session_create_uses_long_running_executor() {
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: "session/create".to_string(),
            params: Some(raw_params(serde_json::json!({
                "prompt": "hello"
            }))),
        };

        assert!(request_semantics(&request).requires_long_running_executor());
    }

    #[test]
    fn tools_register_requires_connection_ordered_dispatch() {
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: "tools/register".to_string(),
            params: Some(raw_params(serde_json::json!({
                "tools": []
            }))),
        };

        assert!(requires_connection_ordered_dispatch(&request));
        assert!(!request_semantics(&request).requires_long_running_executor());
    }

    #[test]
    fn rpc_request_admission_is_process_shared_and_preserves_live_error_shape() {
        let live_request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: "live/send_input".to_string(),
            params: Some(raw_params(serde_json::json!({
                "channel_id": "ch_test",
                "chunk": {
                    "kind": "image",
                    "mime": "image/png",
                    "data": "iVBORw0KGgo="
                }
            }))),
        };
        let admission_capacity = live_send_input_memory_charge_bytes(0);
        let admission = RpcRequestAdmission::new(admission_capacity, 8);
        let admission_from_another_connection = admission.clone();
        let first = admission
            .admit(&live_request)
            .expect("first request admission should remain open");

        let second = admission_from_another_connection
            .admit(&live_request)
            .expect_err("a request on another connection must share process backpressure");
        assert!(matches!(
            &second,
            RpcRequestAdmissionError::Backpressured {
                max_inflight_bytes,
                requested_bytes,
                ..
            } if *max_inflight_bytes == admission_capacity
                && *requested_bytes == admission_capacity
        ));
        let response = rpc_admission_error_response(&live_request, &second);
        assert_eq!(response.id, live_request.id);
        assert_eq!(
            response.error.as_ref().map(|error| error.code),
            Some(crate::error::BUDGET_EXHAUSTED)
        );
        assert_eq!(
            response
                .error
                .as_ref()
                .and_then(|error| error.data.as_ref())
                .and_then(|data| data.get("error_code"))
                .and_then(|code| code.get("reason"))
                .and_then(|reason| reason.get("kind"))
                .and_then(serde_json::Value::as_str),
            Some("input_backpressured")
        );

        let ordinary_request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(2)),
            method: "live/status".to_string(),
            params: Some(raw_params(serde_json::json!({"channel_id": "ch_test"}))),
        };
        assert!(matches!(
            admission.admit(&ordinary_request),
            Err(RpcRequestAdmissionError::Backpressured { .. })
        ));

        drop(first);
        let ordinary = admission
            .admit(&ordinary_request)
            .expect("ordinary request must use the released process budget");
        drop(ordinary);
        assert!(admission.admit(&live_request).is_ok());
    }

    #[test]
    fn production_rpc_admission_is_shared_across_server_constructors() {
        let first = RpcRequestAdmission::production();
        let second = RpcRequestAdmission::production();
        assert!(Arc::ptr_eq(
            &first.memory_semaphore,
            &second.memory_semaphore
        ));
        assert!(Arc::ptr_eq(&first.task_semaphore, &second.task_semaphore));
    }

    #[test]
    fn production_frame_read_admission_is_shared_without_a_four_socket_ceiling() {
        let first = JsonlFrameAdmission::production();
        let second = JsonlFrameAdmission::production();
        assert!(first.shares_process_owner_with(&second));
        const {
            assert!(
                RPC_TCP_MAX_CONNECTIONS > 4,
                "idle socket overhead cap must not preserve the old four-client denial boundary"
            );
        }
    }

    #[test]
    fn live_send_input_admission_charges_the_full_retained_request_shape() {
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Str("request-identity".to_string())),
            method: "live/send_input".to_string(),
            params: Some(raw_params(serde_json::json!({
                "padding": "x".repeat(LIVE_SEND_INPUT_MIN_ADMISSION_BYTES),
            }))),
        };
        let params_bytes = request
            .params
            .as_deref()
            .map(|params| params.get().len())
            .expect("request params");
        let expected = std::mem::size_of::<RpcRequest>()
            + request.jsonrpc.len()
            + request.method.len()
            + "request-identity".len()
            + params_bytes;
        let retained =
            retained_rpc_request_bytes(&request).expect("bounded request id must be chargeable");
        assert_eq!(retained, expected);
        assert!(retained > params_bytes);

        let charged = live_send_input_memory_charge_bytes(retained);
        let admission = RpcRequestAdmission::new(charged - 1, 8);
        assert!(matches!(
            admission.admit(&request),
            Err(RpcRequestAdmissionError::RequestTooLarge {
                max_bytes,
                retained_bytes,
                charged_bytes,
            }) if max_bytes == (charged - 1) / RPC_REQUEST_MEMORY_CHARGE_MULTIPLIER
                && retained_bytes == retained
                && charged_bytes == charged
        ));
    }

    #[test]
    fn live_open_admission_is_process_exclusive_and_releases_on_completion() {
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: "live/open".to_string(),
            params: Some(raw_params(serde_json::json!({
                "session_id": "session_test",
            }))),
        };
        let admission = RpcRequestAdmission::new(RPC_PROCESS_REQUEST_MEMORY_BUDGET_BYTES, 8);
        let admission_from_another_connection = admission.clone();

        let first = admission
            .admit(&request)
            .expect("one image-heavy live/open must fit the process request lane");
        let second = admission_from_another_connection
            .admit(&request)
            .expect_err("a second live/open must fail-fast while hydration is in flight");
        assert!(matches!(
            second,
            RpcRequestAdmissionError::Backpressured {
                max_inflight_bytes: RPC_PROCESS_REQUEST_MEMORY_BUDGET_BYTES,
                requested_bytes: RPC_PROCESS_REQUEST_MEMORY_BUDGET_BYTES,
                ..
            }
        ));

        drop(first);
        assert!(
            admission_from_another_connection.admit(&request).is_ok(),
            "finishing the first live/open must release process admission"
        );
    }

    #[test]
    fn process_budget_admits_one_maximum_valid_live_input_peak() {
        let charged = live_send_input_memory_charge_bytes(RPC_REQUEST_MAX_RETAINED_BYTES);
        assert_eq!(charged, RPC_PROCESS_REQUEST_MEMORY_BUDGET_BYTES);
        assert!(charged <= usize::try_from(u32::MAX).expect("u32 fits usize"));
    }

    #[test]
    fn admitted_response_holds_budget_through_serialization_lifetime() {
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: "live/send_input".to_string(),
            params: Some(raw_params(serde_json::json!({
                "channel_id": "ch_test",
                "chunk": { "kind": "text", "text": "hello" }
            }))),
        };
        let capacity = live_send_input_memory_charge_bytes(0);
        let admission = RpcRequestAdmission::new(capacity, 1);
        let permit = admission.admit(&request).expect("first request admission");
        let admitted = AdmittedRpcResponse {
            response: RpcResponse::success(request.id.clone(), serde_json::json!({"ok": true})),
            _request_permit: permit,
        };

        let serialized = serde_json::to_vec(&admitted.response).expect("serialize response");
        assert!(!serialized.is_empty());
        assert!(matches!(
            admission.admit(&request),
            Err(RpcRequestAdmissionError::Backpressured { .. })
        ));

        drop(admitted);
        assert!(admission.admit(&request).is_ok());
    }

    #[tokio::test]
    async fn tcp_connection_admission_is_held_until_connection_task_finishes() {
        let admission = RpcTcpConnectionAdmission::new(1);
        let first = admission.try_acquire().expect("first connection permit");
        assert!(
            admission.try_acquire().is_err(),
            "a second connection must fail closed without parking"
        );
        drop(first);
        let _released_permit = admission
            .try_acquire()
            .expect("connection permit should release after task lifetime");
    }

    #[test]
    fn oversized_rpc_string_id_is_not_echoed() {
        let oversized_id = format!("private-id:{}", "x".repeat(RPC_MAX_STRING_ID_BYTES));
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Str(oversized_id.clone())),
            method: "live/status".to_string(),
            params: Some(raw_params(serde_json::json!({}))),
        };
        let error = rpc_request_id_bytes(&request)
            .expect_err("oversized string id must be rejected before task spawn");
        assert!(matches!(
            &error,
            RpcRequestAdmissionError::RequestIdTooLarge {
                max_bytes,
                actual_bytes,
            } if *max_bytes == RPC_MAX_STRING_ID_BYTES
                && *actual_bytes == oversized_id.len()
        ));

        let response = rpc_admission_error_response(&request, &error);
        assert!(response.id.is_none(), "oversized id must not be echoed");
        assert_eq!(
            response.error.as_ref().map(|error| error.code),
            Some(crate::error::INVALID_REQUEST)
        );
        assert_eq!(
            response
                .error
                .as_ref()
                .and_then(|error| error.data.as_ref())
                .and_then(|data| data.get("reason"))
                .and_then(serde_json::Value::as_str),
            Some("rpc_request_id_too_large")
        );
        let serialized = serde_json::to_string(&response).expect("error response must serialize");
        assert!(!serialized.contains(&oversized_id));
        assert!(serialized.len() < 1024, "admission error must stay bounded");
    }

    #[test]
    fn oversized_rpc_envelope_is_rejected_without_reflection() {
        let oversized_version = format!(
            "private-version:{}",
            "x".repeat(RPC_MAX_REQUEST_ENVELOPE_BYTES)
        );
        let request = RpcRequest {
            jsonrpc: oversized_version.clone(),
            id: Some(RpcId::Num(9)),
            method: "live/status".to_string(),
            params: None,
        };
        let error = validate_rpc_request_envelope(&request)
            .expect_err("oversized envelope must fail before version reflection or task spawn");
        assert!(matches!(
            error,
            RpcRequestAdmissionError::RequestEnvelopeTooLarge { .. }
        ));
        let response = rpc_admission_error_response(&request, &error);
        assert!(response.id.is_none());
        assert_eq!(
            response.error.as_ref().map(|error| error.code),
            Some(crate::error::INVALID_REQUEST)
        );
        let serialized = serde_json::to_string(&response).expect("response should serialize");
        assert!(!serialized.contains(&oversized_version));
        assert!(
            serialized.len() < 1024,
            "envelope rejection must stay bounded"
        );
    }

    #[test]
    fn cancel_target_parser_bounds_string_identity_before_control_bypass() {
        let oversized = raw_params(serde_json::json!({
            "request_id": "x".repeat(RPC_MAX_STRING_ID_BYTES + 1),
        }));
        assert!(
            oversized.get().len() < RPC_MAX_CANCEL_PARAMS_BYTES,
            "fixture must isolate target-id validation from params validation"
        );
        assert!(request_cancel_target(Some(&oversized)).is_none());

        let valid = raw_params(serde_json::json!({"request_id": "srv-42"}));
        assert_eq!(
            request_cancel_target(Some(&valid)).as_deref(),
            Some("\"srv-42\"")
        );
    }

    #[tokio::test]
    async fn turn_start_is_publish_on_success() {
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: "turn/start".to_string(),
            params: Some(raw_params(serde_json::json!({
                "session_id": "01234567-89ab-cdef-0123-456789abcdef",
                "prompt": "hello"
            }))),
        };

        let semantics = request_semantics(&request);
        assert!(semantics.requires_long_running_executor());
        let executor = SurfaceRequestExecutor::new(tokio::time::Duration::from_millis(1));
        let request_key = request_key(request.id.as_ref().expect("request id"));
        let context = executor.begin_request_with_semantics(
            request_key.clone(),
            noop_request_action(),
            semantics,
        );
        let response = RpcResponse::success(request.id, serde_json::json!({"ok": true}));

        assert!(matches!(
            executor
                .resolve_admitted_terminal(Some(context.key()), response.error.is_none(), response)
                .await,
            RequestTerminalResolution::Emit(_)
        ));
        assert_eq!(executor.phase(&request_key), None);
        assert_eq!(
            executor.cancel_request(&request_key).await,
            meerkat::surface::CancelOutcome::NotFound
        );
    }

    #[derive(Clone)]
    struct SharedBufferWriter(Arc<std::sync::Mutex<Vec<u8>>>);

    #[async_trait]
    impl TransportWriter for SharedBufferWriter {
        async fn write_message(&mut self, bytes: Vec<u8>) -> Result<(), TransportError> {
            let mut output = self
                .0
                .lock()
                .map_err(|_| std::io::Error::other("test writer mutex poisoned"))?;
            output.extend(bytes);
            Ok(())
        }
    }

    #[derive(Clone)]
    struct GateWriter {
        output: Arc<std::sync::Mutex<Vec<u8>>>,
        started: Arc<Semaphore>,
        release: Arc<Semaphore>,
    }

    #[async_trait]
    impl TransportWriter for GateWriter {
        async fn write_message(&mut self, bytes: Vec<u8>) -> Result<(), TransportError> {
            self.started.add_permits(1);
            let release = self
                .release
                .acquire()
                .await
                .map_err(|_| std::io::Error::other("test response gate closed"))?;
            release.forget();
            let mut output = self
                .output
                .lock()
                .map_err(|_| std::io::Error::other("test writer mutex poisoned"))?;
            output.extend(bytes);
            Ok(())
        }
    }

    fn rpc_request_line(request: &RpcRequest) -> Vec<u8> {
        let mut line = serde_json::to_vec(request).expect("request should serialize");
        line.push(b'\n');
        line
    }

    fn rpc_response_line(response: &RpcResponse) -> Vec<u8> {
        let mut line = serde_json::to_vec(response).expect("response should serialize");
        line.push(b'\n');
        line
    }

    async fn wait_for_rpc_responses(
        output: &Arc<std::sync::Mutex<Vec<u8>>>,
        expected: usize,
    ) -> Vec<RpcResponse> {
        tokio::time::timeout(tokio::time::Duration::from_secs(2), async {
            loop {
                let bytes = output.lock().expect("output lock").clone();
                let responses = String::from_utf8(bytes)
                    .expect("response should be utf8")
                    .lines()
                    .map(|line| serde_json::from_str(line).expect("response should parse"))
                    .collect::<Vec<RpcResponse>>();
                if responses.len() >= expected {
                    return responses;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("timed out waiting for RPC responses")
    }

    fn run_stack_heavy_rpc_loop_test(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .name("rpc-loop-test".to_string())
            .stack_size(16 * 1024 * 1024)
            .spawn(test)
            .expect("RPC loop test thread should spawn")
            .join()
            .expect("RPC loop test thread should not panic");
    }

    #[test]
    fn rpc_admission_is_cross_connection_nonblocking_and_releases_after_write() {
        run_stack_heavy_rpc_loop_test(|| {
            let test_runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            test_runtime.block_on(async {
                let temp = tempfile::tempdir().unwrap();
                let (runtime, config_store) = build_test_runtime(&temp);
                let ordinary_request = RpcRequest {
                    jsonrpc: "2.0".to_string(),
                    id: Some(RpcId::Num(1)),
                    method: "test/ordinary-not-found".to_string(),
                    params: Some(raw_params(serde_json::json!({}))),
                };
                let retained = retained_rpc_request_bytes(&ordinary_request).unwrap();
                let shared_admission = RpcRequestAdmission::new(
                    rpc_request_memory_charge_bytes(&ordinary_request, retained),
                    1,
                );

                let (mut client_one, server_one_stream) = tokio::io::duplex(16 * 1024);
                let first_output = Arc::new(std::sync::Mutex::new(Vec::new()));
                let write_started = Arc::new(Semaphore::new(0));
                let write_release = Arc::new(Semaphore::new(0));
                let first_writer = GateWriter {
                    output: Arc::clone(&first_output),
                    started: Arc::clone(&write_started),
                    release: Arc::clone(&write_release),
                };
                let mut server_one = Box::new(
                    RpcServer::new(
                        TokioBufReader::new(server_one_stream),
                        first_writer,
                        Arc::clone(&runtime),
                        Arc::clone(&config_store),
                    )
                    .with_rpc_request_admission(shared_admission.clone()),
                );
                server_one.skip_shutdown_on_eof = true;
                let server_one_task = tokio::spawn(async move { server_one.run().await });

                let (mut client_two, server_two_stream) = tokio::io::duplex(16 * 1024);
                let second_output = Arc::new(std::sync::Mutex::new(Vec::new()));
                let mut server_two = Box::new(
                    RpcServer::new(
                        TokioBufReader::new(server_two_stream),
                        SharedBufferWriter(Arc::clone(&second_output)),
                        runtime,
                        config_store,
                    )
                    .with_rpc_request_admission(shared_admission.clone()),
                );
                server_two.skip_shutdown_on_eof = true;
                let server_two_task = tokio::spawn(async move { server_two.run().await });

                client_one
                    .write_all(&rpc_request_line(&ordinary_request))
                    .await
                    .unwrap();
                let started = tokio::time::timeout(
                    tokio::time::Duration::from_secs(2),
                    write_started.acquire(),
                )
                .await
                .expect("ordinary response write should start")
                .expect("write-start semaphore should remain open");
                started.forget();

                // Exercise the long-running branch while an ordinary request on a
                // different connection owns the sole task/byte reservation. This must
                // reject immediately instead of spawning or parking a waiter.
                let long_running_request = RpcRequest {
                    jsonrpc: "2.0".to_string(),
                    id: Some(RpcId::Num(2)),
                    method: "turn/start".to_string(),
                    params: Some(raw_params(serde_json::json!({
                        "session_id": "01234567-89ab-cdef-0123-456789abcdef",
                        "prompt": "must not dispatch while saturated"
                    }))),
                };
                client_two
                    .write_all(&rpc_request_line(&long_running_request))
                    .await
                    .unwrap();
                let responses = wait_for_rpc_responses(&second_output, 1).await;
                assert_eq!(
                    responses[0].error.as_ref().map(|error| error.code),
                    Some(crate::error::BUDGET_EXHAUSTED)
                );
                assert_eq!(
                    responses[0]
                        .error
                        .as_ref()
                        .and_then(|error| error.data.as_ref())
                        .and_then(|data| data.get("reason"))
                        .and_then(serde_json::Value::as_str),
                    Some("rpc_request_backpressured")
                );
                assert_eq!(
                    shared_admission.task_semaphore.available_permits(),
                    0,
                    "saturated rejection must not park a waiter or steal a permit"
                );

                write_release.add_permits(1);
                let _ = wait_for_rpc_responses(&first_output, 1).await;
                tokio::time::timeout(tokio::time::Duration::from_secs(2), async {
                    while shared_admission.task_semaphore.available_permits() != 1 {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("request admission should release after transport write returns");

                let retry_request = RpcRequest {
                    id: Some(RpcId::Num(3)),
                    ..ordinary_request
                };
                client_two
                    .write_all(&rpc_request_line(&retry_request))
                    .await
                    .unwrap();
                let responses = wait_for_rpc_responses(&second_output, 2).await;
                assert_eq!(responses[1].id, Some(RpcId::Num(3)));
                assert_eq!(
                    responses[1].error.as_ref().map(|error| error.code),
                    Some(crate::error::METHOD_NOT_FOUND)
                );

                server_one_task.abort();
                server_two_task.abort();
            });
        });
    }

    #[test]
    fn idle_rpc_connection_does_not_reserve_active_frame_capacity_or_starve_peer() {
        run_stack_heavy_rpc_loop_test(|| {
            let test_runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            test_runtime.block_on(async {
                let temp = tempfile::tempdir().unwrap();
                let (runtime, config_store) = build_test_runtime(&temp);
                let small_budget = JSONL_SMALL_FRAME_BYTES * 3;
                let frame_admission = JsonlFrameAdmission::new(small_budget, 0);

                let (_idle_client, idle_server_stream) = tokio::io::duplex(16 * 1024);
                let mut idle_server = Box::new(
                    RpcServer::new(
                        TokioBufReader::new(idle_server_stream),
                        SharedBufferWriter(Arc::new(std::sync::Mutex::new(Vec::new()))),
                        Arc::clone(&runtime),
                        Arc::clone(&config_store),
                    )
                    .with_rpc_frame_read_admission(frame_admission.clone()),
                );
                idle_server.skip_shutdown_on_eof = true;
                let idle_task = tokio::spawn(async move { idle_server.run().await });
                tokio::task::yield_now().await;
                assert_eq!(
                    frame_admission.available_small_bytes(),
                    small_budget,
                    "an idle connection must wait for a buffered byte before frame admission"
                );

                let (mut active_client, active_server_stream) = tokio::io::duplex(16 * 1024);
                let active_output = Arc::new(std::sync::Mutex::new(Vec::new()));
                let mut active_server = Box::new(
                    RpcServer::new(
                        TokioBufReader::new(active_server_stream),
                        SharedBufferWriter(Arc::clone(&active_output)),
                        runtime,
                        config_store,
                    )
                    .with_rpc_frame_read_admission(frame_admission.clone()),
                );
                active_server.skip_shutdown_on_eof = true;
                let active_task = tokio::spawn(async move { active_server.run().await });

                let request = RpcRequest {
                    jsonrpc: "1.0".to_string(),
                    id: Some(RpcId::Num(1)),
                    method: "test/readable-peer".to_string(),
                    params: None,
                };
                active_client
                    .write_all(&rpc_request_line(&request))
                    .await
                    .unwrap();
                let responses = wait_for_rpc_responses(&active_output, 1).await;
                assert_eq!(responses[0].id, Some(RpcId::Num(1)));
                assert_eq!(
                    responses[0].error.as_ref().map(|error| error.code),
                    Some(crate::error::INVALID_REQUEST)
                );

                idle_task.abort();
                active_task.abort();
            });
        });
    }

    #[test]
    fn continuously_readable_peer_cannot_starve_queued_outbound_response() {
        run_stack_heavy_rpc_loop_test(|| {
            let test_runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            test_runtime.block_on(async {
                use tokio::io::AsyncWriteExt as _;

                let temp = tempfile::tempdir().unwrap();
                let (runtime, config_store) = build_test_runtime(&temp);
                let (mut client, server_stream) = tokio::io::duplex(256 * 1024);
                let output = Arc::new(std::sync::Mutex::new(Vec::new()));
                let mut server = Box::new(RpcServer::new(
                    TokioBufReader::new(server_stream),
                    SharedBufferWriter(Arc::clone(&output)),
                    runtime,
                    config_store,
                ));
                server.skip_shutdown_on_eof = true;

                let request = RpcRequest {
                    jsonrpc: "2.0".to_string(),
                    id: Some(RpcId::Num(99)),
                    method: "test/fairness".to_string(),
                    params: None,
                };
                let request_permit = server
                    .rpc_request_admission
                    .admit(&request)
                    .expect("fairness response request admission");
                server
                    .response_tx
                    .try_send(AdmittedRpcResponse {
                        response: RpcResponse::success(
                            request.id.clone(),
                            serde_json::json!({"fair": true}),
                        ),
                        _request_permit: request_permit,
                    })
                    .expect("queue fairness response before starting loop");
                let server_task = tokio::spawn(async move { server.run().await });

                let cancel = RpcRequest {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    method: "cancel".to_string(),
                    params: None,
                };
                let cancel_line = rpc_request_line(&cancel);
                let flood_task = tokio::spawn(async move {
                    loop {
                        if client.write_all(&cancel_line).await.is_err() {
                            break;
                        }
                    }
                });

                let responses = wait_for_rpc_responses(&output, 1).await;
                assert_eq!(responses[0].id, Some(RpcId::Num(99)));
                assert!(responses[0].error.is_none());
                flood_task.abort();
                server_task.abort();
            });
        });
    }

    #[test]
    fn rpc_loop_bounds_oversized_methods_and_never_replies_to_invalid_notifications() {
        run_stack_heavy_rpc_loop_test(|| {
            let test_runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            test_runtime.block_on(async {
                let temp = tempfile::tempdir().unwrap();
                let (runtime, config_store) = build_test_runtime(&temp);
                let (mut client, server_stream) = tokio::io::duplex(32 * 1024);
                let output = Arc::new(std::sync::Mutex::new(Vec::new()));
                let admission = RpcRequestAdmission::new(
                    RPC_PROCESS_REQUEST_MEMORY_BUDGET_BYTES,
                    RPC_PROCESS_MAX_INFLIGHT_REQUESTS,
                );
                let mut server = Box::new(
                    RpcServer::new(
                        TokioBufReader::new(server_stream),
                        SharedBufferWriter(Arc::clone(&output)),
                        runtime,
                        config_store,
                    )
                    .with_rpc_request_admission(admission.clone()),
                );
                server.skip_shutdown_on_eof = true;
                let server_task = tokio::spawn(async move { server.run().await });

                let oversized_method =
                    format!("private-method:{}", "x".repeat(RPC_MAX_METHOD_BYTES));
                let invalid_request = RpcRequest {
                    jsonrpc: "2.0".to_string(),
                    id: Some(RpcId::Num(1)),
                    method: oversized_method.clone(),
                    params: None,
                };
                let invalid_notification = RpcRequest {
                    id: None,
                    ..invalid_request.clone()
                };
                let valid_request = RpcRequest {
                    jsonrpc: "2.0".to_string(),
                    id: Some(RpcId::Num(2)),
                    method: "test/not-found-after-invalid-notification".to_string(),
                    params: None,
                };
                client
                    .write_all(&rpc_request_line(&invalid_request))
                    .await
                    .unwrap();
                client
                    .write_all(&rpc_request_line(&invalid_notification))
                    .await
                    .unwrap();
                client
                    .write_all(&rpc_request_line(&valid_request))
                    .await
                    .unwrap();

                let responses = wait_for_rpc_responses(&output, 2).await;
                assert_eq!(
                    responses.len(),
                    2,
                    "notification must not receive a response"
                );
                assert!(
                    responses[0].id.is_none(),
                    "oversized method must not be echoed"
                );
                assert_eq!(
                    responses[0].error.as_ref().map(|error| error.code),
                    Some(crate::error::INVALID_REQUEST)
                );
                assert_eq!(responses[1].id, Some(RpcId::Num(2)));
                let serialized = output.lock().expect("output lock").clone();
                assert!(
                    !String::from_utf8(serialized)
                        .expect("response utf8")
                        .contains(&oversized_method),
                    "bounded rejection must not reflect the attacker-controlled method"
                );
                tokio::time::timeout(tokio::time::Duration::from_secs(2), async {
                    while admission.task_semaphore.available_permits()
                        != RPC_PROCESS_MAX_INFLIGHT_REQUESTS
                    {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("valid response write should release request admission");

                server_task.abort();
            });
        });
    }

    #[tokio::test]
    async fn cancel_bypass_is_strictly_bounded_and_remains_live_under_data_saturation() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);
        let barrier = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: "test/cancel-barrier".to_string(),
            params: None,
        };
        let retained = retained_rpc_request_bytes(&barrier).unwrap();
        let admission =
            RpcRequestAdmission::new(rpc_request_memory_charge_bytes(&barrier, retained), 1);
        let held_data_permit = admission
            .admit(&barrier)
            .expect("test should saturate the sole data-plane reservation");

        let (mut client, server_stream) = tokio::io::duplex(32 * 1024);
        let output = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut server = Box::new(
            RpcServer::new(
                TokioBufReader::new(server_stream),
                SharedBufferWriter(Arc::clone(&output)),
                runtime,
                config_store,
            )
            .with_rpc_request_admission(admission.clone()),
        );
        server.skip_shutdown_on_eof = true;
        let tracked_id = RpcId::Num(77);
        let tracked_key = request_key(&tracked_id);
        let _tracked = server.request_executor.begin_request_with_semantics(
            tracked_key.clone(),
            noop_request_action(),
            SurfaceRequestSemantics::long_running_publish_on_success(),
        );
        let request_executor = server.request_executor.clone();
        let server_task = tokio::spawn(async move { server.run().await });

        let oversized_cancel = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: "cancel".to_string(),
            params: Some(raw_params(serde_json::json!({
                "request_id": 77,
                "padding": "x".repeat(RPC_MAX_CANCEL_PARAMS_BYTES + 1),
            }))),
        };
        client
            .write_all(&rpc_request_line(&oversized_cancel))
            .await
            .unwrap();
        client.write_all(&rpc_request_line(&barrier)).await.unwrap();
        let responses = wait_for_rpc_responses(&output, 1).await;
        assert_eq!(
            responses[0].error.as_ref().map(|error| error.code),
            Some(crate::error::BUDGET_EXHAUSTED)
        );
        assert_eq!(
            request_executor.phase(&tracked_key),
            Some(meerkat::surface::SurfaceRequestPhase::Pending),
            "oversized cancel params must be dropped before parse/cancel"
        );
        assert_eq!(admission.task_semaphore.available_permits(), 0);

        let valid_cancel = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: "cancel".to_string(),
            params: Some(raw_params(serde_json::json!({"request_id": 77}))),
        };
        let second_barrier = RpcRequest {
            id: Some(RpcId::Num(2)),
            ..barrier
        };
        client
            .write_all(&rpc_request_line(&valid_cancel))
            .await
            .unwrap();
        client
            .write_all(&rpc_request_line(&second_barrier))
            .await
            .unwrap();
        let responses = wait_for_rpc_responses(&output, 2).await;
        assert_eq!(
            responses.len(),
            2,
            "cancel notifications remain response-free"
        );
        assert_eq!(responses[1].id, Some(RpcId::Num(2)));
        assert_eq!(
            request_executor.phase(&tracked_key),
            Some(meerkat::surface::SurfaceRequestPhase::Cancelled),
            "valid cancel must bypass saturated data admission"
        );
        assert_eq!(admission.task_semaphore.available_permits(), 0);

        drop(held_data_permit);
        server_task.abort();
    }

    #[test]
    fn production_callback_response_admission_is_process_shared() {
        let first = RpcCallbackResponseAdmission::production();
        let second = RpcCallbackResponseAdmission::production();
        assert!(Arc::ptr_eq(
            &first.memory_semaphore,
            &second.memory_semaphore
        ));
        assert!(Arc::ptr_eq(&first.count_semaphore, &second.count_semaphore));
    }

    #[tokio::test]
    async fn callback_response_admission_crosses_oneshot_and_releases_after_handoff_drop() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);
        let callback_id = RpcId::Str("srv-admission-test".to_string());
        let response = RpcResponse::success(
            Some(callback_id.clone()),
            serde_json::json!({"content": "hello", "is_error": false}),
        );
        let retained = retained_rpc_callback_response_bytes(&response);
        let charged = retained
            .max(RPC_CALLBACK_RESPONSE_MIN_ADMISSION_BYTES)
            .saturating_mul(RPC_CALLBACK_RESPONSE_MEMORY_CHARGE_MULTIPLIER);
        let admission = RpcCallbackResponseAdmission::new(charged, 1, retained);

        let (mut client, server_stream) = tokio::io::duplex(16 * 1024);
        let mut server = Box::new(
            RpcServer::new(
                TokioBufReader::new(server_stream),
                SharedBufferWriter(Arc::new(std::sync::Mutex::new(Vec::new()))),
                runtime,
                config_store,
            )
            .with_callback_response_admission(admission.clone()),
        );
        server.skip_shutdown_on_eof = true;
        let (response_tx, response_rx) = oneshot::channel();
        server
            .pending_callbacks
            .insert(callback_id.clone(), response_tx);
        let server_task = tokio::spawn(async move { server.run().await });

        client
            .write_all(&rpc_response_line(&response))
            .await
            .unwrap();
        let handoff = tokio::time::timeout(tokio::time::Duration::from_secs(2), response_rx)
            .await
            .expect("callback response should reach oneshot")
            .expect("callback response sender should remain open");
        assert_eq!(handoff.response().id, Some(callback_id));
        assert_eq!(admission.memory_semaphore.available_permits(), 0);
        assert_eq!(admission.count_semaphore.available_permits(), 0);
        let response_wire_bytes = rpc_response_line(&response).len().saturating_sub(1);
        assert!(matches!(
            admission.admit(response, response_wire_bytes),
            Err(RpcCallbackResponseAdmissionError::Backpressured { .. })
        ));

        drop(handoff);
        assert_eq!(admission.memory_semaphore.available_permits(), charged);
        assert_eq!(admission.count_semaphore.available_permits(), 1);
        server_task.abort();
    }

    #[tokio::test]
    async fn oversized_callback_response_becomes_bounded_rejection_without_permit_or_waiter() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);
        let callback_id = RpcId::Str("srv-oversized-test".to_string());
        let admission = RpcCallbackResponseAdmission::new(1024, 1, 256);
        let response = RpcResponse::success(
            Some(callback_id.clone()),
            serde_json::json!({"content": "x".repeat(1024), "is_error": false}),
        );

        let (mut client, server_stream) = tokio::io::duplex(16 * 1024);
        let mut server = Box::new(
            RpcServer::new(
                TokioBufReader::new(server_stream),
                SharedBufferWriter(Arc::new(std::sync::Mutex::new(Vec::new()))),
                runtime,
                config_store,
            )
            .with_callback_response_admission(admission.clone()),
        );
        server.skip_shutdown_on_eof = true;
        let (response_tx, response_rx) = oneshot::channel();
        server
            .pending_callbacks
            .insert(callback_id.clone(), response_tx);
        let server_task = tokio::spawn(async move { server.run().await });

        client
            .write_all(&rpc_response_line(&response))
            .await
            .unwrap();
        let handoff = tokio::time::timeout(tokio::time::Duration::from_secs(2), response_rx)
            .await
            .expect("oversized callback must reject without parking")
            .expect("bounded rejection should reach callback consumer");
        let rejection = handoff.response();
        assert_eq!(rejection.id, Some(callback_id));
        assert_eq!(
            rejection.error.as_ref().map(|error| error.code),
            Some(crate::error::INVALID_REQUEST)
        );
        assert!(rejection.result.is_none());
        assert!(
            serde_json::to_vec(rejection)
                .expect("rejection should serialize")
                .len()
                < 1024
        );
        assert_eq!(admission.memory_semaphore.available_permits(), 1024);
        assert_eq!(admission.count_semaphore.available_permits(), 1);

        server_task.abort();
    }

    #[tokio::test]
    async fn publish_completion_after_cancel_writes_cancel_response() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);
        let output = Arc::new(std::sync::Mutex::new(Vec::new()));
        let reader = BufReader::new(std::io::Cursor::new(Vec::new()));
        let writer = SharedBufferWriter(Arc::clone(&output));
        let mut server = RpcServer::new(reader, writer, runtime, config_store);
        let request_id = RpcId::Num(42);
        let request_key = request_key(&request_id);
        let _ctx = server.request_executor.begin_request_with_semantics(
            request_key.clone(),
            noop_request_action(),
            SurfaceRequestSemantics::long_running_publish_on_success(),
        );

        assert_eq!(
            server.request_executor.cancel_request(&request_key).await,
            meerkat::surface::CancelOutcome::Cancelled
        );

        let publish_response =
            RpcResponse::success(Some(request_id.clone()), serde_json::json!({"ok": true}));
        let request_permit = server
            .rpc_request_admission
            .admit(&RpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(request_id.clone()),
                method: "turn/start".to_string(),
                params: None,
            })
            .expect("test long-running response admission");
        assert!(
            server
                .write_long_running_response(LongRunningResponse {
                    request_key: request_key.clone(),
                    success: publish_response.error.is_none(),
                    response: publish_response,
                    _request_permit: request_permit,
                })
                .await
        );

        assert_eq!(server.request_executor.phase(&request_key), None);
        let bytes = output.lock().expect("output lock").clone();
        let line = String::from_utf8(bytes).expect("output should be utf8");
        let response: RpcResponse =
            serde_json::from_str(line.trim()).expect("response should parse");
        assert_eq!(response.id, Some(request_id));
        assert!(response.result.is_none());
        assert_eq!(
            response.error.expect("expected cancel error").code,
            crate::error::REQUEST_CANCELLED
        );
    }

    // -----------------------------------------------------------------------
    // TCP integration tests
    // -----------------------------------------------------------------------

    use std::pin::Pin;
    use std::sync::Arc;

    use async_trait::async_trait;
    use futures::{SinkExt, StreamExt, stream};
    use meerkat::{AgentBuildConfig, AgentFactory};
    use meerkat_client::{LlmClient, LlmError};
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::RunPrimitive;
    use meerkat_core::session::ToolCategoryOverride;
    use meerkat_core::{Config, ConfigRuntime, MemoryConfigStore, StopReason};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
    use tokio::net::TcpStream;

    /// Mock LLM that immediately returns "Hello from mock" and ends the turn.
    struct MockLlmClient;

    #[async_trait]
    impl LlmClient for MockLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            Box::pin(stream::iter(vec![
                Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Hello from mock".to_string(),
                    meta: None,
                }),
                Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    },
                }),
            ]))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    fn memory_blob_store() -> Arc<dyn meerkat_core::BlobStore> {
        Arc::new(meerkat_store::MemoryBlobStore::new())
    }

    /// Build a `SessionRuntime` + `ConfigStore` pair for TCP tests.
    fn build_test_runtime(
        temp: &tempfile::TempDir,
    ) -> (Arc<SessionRuntime>, Arc<dyn meerkat_core::ConfigStore>) {
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = Config::default();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime = SessionRuntime::new(
            factory,
            config,
            10,
            meerkat::PersistenceBundle::new(
                store,
                Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
                memory_blob_store(),
            ),
            crate::router::NotificationSink::noop(),
        );
        let config_store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(MemoryConfigStore::new(
            Config::default(),
            meerkat_models::canonical(),
        ));
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        (Arc::new(runtime), config_store)
    }

    struct NeverAppliedExecutor;

    #[async_trait]
    impl CoreExecutor for NeverAppliedExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            unreachable!("tests do not drive executor apply")
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }
}
