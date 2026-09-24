//! JSON-RPC 2.0 message types.
//!
//! Uses `Box<RawValue>` for params/result to avoid early parsing,
//! following the project guideline about pass-through JSON.

use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// The only JSON-RPC protocol version this server speaks.
///
/// Per the JSON-RPC 2.0 spec (and `docs/api/rpc.mdx`), every request and
/// response envelope MUST carry `"jsonrpc": "2.0"`. The transport carries the
/// field as a free `String`, so the version is validated at the dispatch
/// boundary rather than trusted as ambient truth.
pub const JSONRPC_VERSION: &str = "2.0";

/// Hard ceiling for one server-originated JSON-RPC message. A 20 MiB image
/// expands to roughly 27 MiB as base64, so 32 MiB preserves that supported
/// shape while preventing list/snapshot handlers from creating unbounded
/// queued responses.
pub(crate) const RPC_OUTBOUND_MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

/// Notifications are streaming control/data-plane messages and should stay
/// materially smaller than request/response blobs. Large durable projections
/// must be paged or fetched explicitly instead of being pushed into the event
/// queue.
pub(crate) const RPC_OUTBOUND_MAX_NOTIFICATION_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const RPC_COLLECTION_DEFAULT_LIMIT: usize = 100;
pub(crate) const RPC_COLLECTION_MAX_LIMIT: usize = 1000;
pub(crate) const RPC_COLLECTION_MAX_OFFSET: usize = 1_000_000;

const RPC_OUTBOUND_ENVELOPE_SLACK_BYTES: usize = 4 * 1024;
const RPC_OUTBOUND_MIN_CHARGE_BYTES: usize = 64 * 1024;
const RPC_OUTBOUND_MEMORY_MULTIPLIER: usize = 4;
const RPC_OUTBOUND_PROCESS_MEMORY_BUDGET_BYTES: usize = 256 * 1024 * 1024;
const RPC_OUTBOUND_PROCESS_MAX_INFLIGHT: usize = 256;
const RPC_OUTBOUND_MAX_ERROR_DATA_BYTES: usize = 1024 * 1024;
const RPC_OUTBOUND_MAX_ERROR_MESSAGE_BYTES: usize = 4 * 1024;
const RPC_OUTBOUND_MAX_METHOD_BYTES: usize = 256;
const RPC_OUTBOUND_MAX_ID_BYTES: usize = 1024;
pub(crate) const RPC_CONTROL_ERROR_MAX_WIRE_BYTES: usize = 10 * 1024;
pub(crate) const RPC_CONTROL_ERROR_MAX_INFLIGHT: usize = 512;
pub(crate) const RPC_CONTROL_ERROR_WORST_CASE_BYTES: usize =
    RPC_CONTROL_ERROR_MAX_WIRE_BYTES * RPC_CONTROL_ERROR_MAX_INFLIGHT;

/// Process-owned byte/count admission shared by responses, notifications, and
/// callback requests across every RPC listener and stdio surface.
///
/// Admission happens after an allocation-free sizing pass and before the
/// `RawValue`/queue-owned representation is allocated. The permit then travels
/// with the wire envelope until the transport write completes.
#[derive(Clone)]
pub(crate) struct RpcOutboundAdmission {
    memory_semaphore: Arc<Semaphore>,
    count_semaphore: Arc<Semaphore>,
    capacity_bytes: usize,
    max_inflight: usize,
}

#[derive(Debug)]
pub(crate) struct RpcOutboundPermit {
    _memory_permit: OwnedSemaphorePermit,
    _count_permit: OwnedSemaphorePermit,
    wire_bytes: usize,
}

impl RpcOutboundPermit {
    pub(crate) fn wire_bytes(&self) -> usize {
        self.wire_bytes
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RpcOutboundAdmissionError {
    #[error("outbound JSON payload exceeds the {max_bytes}-byte limit ({actual_bytes} bytes)")]
    TooLarge {
        max_bytes: usize,
        actual_bytes: usize,
    },
    #[error("failed to serialize outbound JSON: {0}")]
    Serialize(String),
    #[error(
        "outbound RPC admission is saturated ({requested_bytes} bytes requested; {capacity_bytes} bytes/{max_inflight} messages available process-wide)"
    )]
    Backpressured {
        requested_bytes: usize,
        capacity_bytes: usize,
        max_inflight: usize,
    },
    #[error("outbound RPC admission permit range exceeded")]
    PermitRange,
    #[error("outbound RPC admission is closed")]
    Closed,
}

impl RpcOutboundAdmission {
    pub(crate) fn production() -> Self {
        static PROCESS_ADMISSION: OnceLock<RpcOutboundAdmission> = OnceLock::new();
        PROCESS_ADMISSION
            .get_or_init(|| {
                Self::new(
                    RPC_OUTBOUND_PROCESS_MEMORY_BUDGET_BYTES,
                    RPC_OUTBOUND_PROCESS_MAX_INFLIGHT,
                )
            })
            .clone()
    }

    pub(crate) fn new(capacity_bytes: usize, max_inflight: usize) -> Self {
        Self {
            memory_semaphore: Arc::new(Semaphore::new(capacity_bytes)),
            count_semaphore: Arc::new(Semaphore::new(max_inflight)),
            capacity_bytes,
            max_inflight,
        }
    }

    /// Size, admit, and serialize a JSON payload without allocating any
    /// queue-owned bytes before the process reservation succeeds.
    pub(crate) fn admit_json<T: Serialize + ?Sized>(
        &self,
        value: &T,
        max_payload_bytes: usize,
        envelope_bytes: usize,
    ) -> Result<(Box<RawValue>, Arc<RpcOutboundPermit>), RpcOutboundAdmissionError> {
        let payload_bytes = bounded_json_size(value, max_payload_bytes)?;
        let wire_bytes = payload_bytes.saturating_add(envelope_bytes);
        let permit = self.try_admit(wire_bytes)?;
        let raw = serialize_raw_value_exact(value, payload_bytes)?;
        Ok((raw, permit))
    }

    pub(crate) fn try_admit(
        &self,
        wire_bytes: usize,
    ) -> Result<Arc<RpcOutboundPermit>, RpcOutboundAdmissionError> {
        if wire_bytes > RPC_OUTBOUND_MAX_MESSAGE_BYTES {
            return Err(RpcOutboundAdmissionError::TooLarge {
                max_bytes: RPC_OUTBOUND_MAX_MESSAGE_BYTES,
                actual_bytes: wire_bytes,
            });
        }
        let charged_bytes = wire_bytes
            .max(RPC_OUTBOUND_MIN_CHARGE_BYTES)
            .saturating_mul(RPC_OUTBOUND_MEMORY_MULTIPLIER);
        if charged_bytes > self.capacity_bytes {
            return Err(RpcOutboundAdmissionError::TooLarge {
                max_bytes: self.capacity_bytes / RPC_OUTBOUND_MEMORY_MULTIPLIER,
                actual_bytes: wire_bytes,
            });
        }
        let permits =
            u32::try_from(charged_bytes).map_err(|_| RpcOutboundAdmissionError::PermitRange)?;
        let count_permit = match Arc::clone(&self.count_semaphore).try_acquire_owned() {
            Ok(permit) => permit,
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                return Err(RpcOutboundAdmissionError::Backpressured {
                    requested_bytes: charged_bytes,
                    capacity_bytes: self.capacity_bytes,
                    max_inflight: self.max_inflight,
                });
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                return Err(RpcOutboundAdmissionError::Closed);
            }
        };
        let memory_permit = match Arc::clone(&self.memory_semaphore).try_acquire_many_owned(permits)
        {
            Ok(permit) => permit,
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                return Err(RpcOutboundAdmissionError::Backpressured {
                    requested_bytes: charged_bytes,
                    capacity_bytes: self.capacity_bytes,
                    max_inflight: self.max_inflight,
                });
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                return Err(RpcOutboundAdmissionError::Closed);
            }
        };
        Ok(Arc::new(RpcOutboundPermit {
            _memory_permit: memory_permit,
            _count_permit: count_permit,
            wire_bytes,
        }))
    }

    #[cfg(test)]
    fn available_memory_bytes(&self) -> usize {
        self.memory_semaphore.available_permits()
    }

    #[cfg(test)]
    fn available_count(&self) -> usize {
        self.count_semaphore.available_permits()
    }
}

#[derive(Default)]
struct JsonSizeWriter {
    written: usize,
    max_bytes: usize,
    exceeded: bool,
}

impl std::io::Write for JsonSizeWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = self.written.saturating_add(bytes.len());
        if next > self.max_bytes {
            self.exceeded = true;
            self.written = next;
            return Err(std::io::Error::other("outbound JSON size limit exceeded"));
        }
        self.written = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn bounded_json_size<T: Serialize + ?Sized>(
    value: &T,
    max_bytes: usize,
) -> Result<usize, RpcOutboundAdmissionError> {
    let mut writer = JsonSizeWriter {
        max_bytes,
        ..JsonSizeWriter::default()
    };
    match serde_json::to_writer(&mut writer, value) {
        Ok(()) => Ok(writer.written),
        Err(_) if writer.exceeded => Err(RpcOutboundAdmissionError::TooLarge {
            max_bytes,
            actual_bytes: writer.written,
        }),
        Err(error) => Err(RpcOutboundAdmissionError::Serialize(error.to_string())),
    }
}

struct BoundedVecWriter {
    bytes: Vec<u8>,
    max_bytes: usize,
}

impl std::io::Write for BoundedVecWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.max_bytes {
            return Err(std::io::Error::other(
                "outbound JSON changed size between admission and serialization",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn serialize_raw_value_exact<T: Serialize + ?Sized>(
    value: &T,
    admitted_bytes: usize,
) -> Result<Box<RawValue>, RpcOutboundAdmissionError> {
    let mut writer = BoundedVecWriter {
        bytes: Vec::with_capacity(admitted_bytes),
        max_bytes: admitted_bytes,
    };
    serde_json::to_writer(&mut writer, value)
        .map_err(|error| RpcOutboundAdmissionError::Serialize(error.to_string()))?;
    let json = String::from_utf8(writer.bytes)
        .map_err(|error| RpcOutboundAdmissionError::Serialize(error.to_string()))?;
    RawValue::from_string(json)
        .map_err(|error| RpcOutboundAdmissionError::Serialize(error.to_string()))
}

fn truncate_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}

fn response_envelope_bytes(id: Option<&RpcId>) -> usize {
    let id_bytes = match id {
        Some(RpcId::Num(_)) => 20,
        Some(RpcId::Str(id)) => id.len(),
        None => 4,
    };
    RPC_OUTBOUND_ENVELOPE_SLACK_BYTES.saturating_add(id_bytes)
}

fn bounded_outbound_id(id: Option<RpcId>) -> Option<RpcId> {
    match id {
        Some(RpcId::Str(id)) if id.len() > RPC_OUTBOUND_MAX_ID_BYTES => None,
        other => other,
    }
}

pub(crate) fn bounded_control_error_bytes(
    connection_slots: usize,
    request_slots: usize,
    callback_slots: usize,
) -> Option<usize> {
    let inflight = connection_slots
        .checked_add(request_slots)?
        .checked_add(callback_slots)?;
    (inflight <= RPC_CONTROL_ERROR_MAX_INFLIGHT)
        .then(|| inflight.saturating_mul(RPC_CONTROL_ERROR_MAX_WIRE_BYTES))
}

pub(crate) fn bounded_collection_limit(requested: Option<usize>) -> Result<usize, String> {
    let limit = requested.unwrap_or(RPC_COLLECTION_DEFAULT_LIMIT);
    if limit > RPC_COLLECTION_MAX_LIMIT {
        return Err(format!(
            "limit {limit} exceeds the RPC collection maximum of {RPC_COLLECTION_MAX_LIMIT}"
        ));
    }
    Ok(limit)
}

pub(crate) fn bounded_collection_offset(requested: Option<usize>) -> Result<usize, String> {
    let offset = requested.unwrap_or(0);
    if offset > RPC_COLLECTION_MAX_OFFSET {
        return Err(format!(
            "offset {offset} exceeds the RPC collection maximum of {RPC_COLLECTION_MAX_OFFSET}"
        ));
    }
    Ok(offset)
}

/// JSON-RPC request or notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<RpcId>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Box<RawValue>>,
}

/// JSON-RPC message identifier (number or string).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcId {
    Num(i64),
    Str(String),
}

/// JSON-RPC response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: Option<RpcId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Box<RawValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
    /// Process-wide outbound memory/count owner. Inbound callback responses
    /// deserialize with no permit; server-originated constructors install one
    /// before allocating a queue-owned result.
    #[serde(skip)]
    outbound_permit: Option<Arc<RpcOutboundPermit>>,
}

/// JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// JSON-RPC notification (server -> client, no id, no response expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Box<RawValue>,
    #[serde(skip)]
    outbound_permit: Option<Arc<RpcOutboundPermit>>,
}

impl RpcRequest {
    /// Returns true if this is a notification (no id).
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }

    /// Returns true iff the envelope declares the supported JSON-RPC version.
    ///
    /// The version is validated at the dispatch boundary; a frame whose
    /// `jsonrpc` is missing/empty (deserialized to a non-`"2.0"` string) or set
    /// to any other value is rejected with the standard `INVALID_REQUEST` error
    /// rather than dispatched.
    pub fn has_supported_version(&self) -> bool {
        self.jsonrpc == JSONRPC_VERSION
    }
}

/// Discriminated union of incoming messages: either a request or a response.
///
/// The transport reads raw JSON and disambiguates by field presence:
/// messages with a `method` field are requests; others are responses.
#[derive(Debug)]
pub enum RpcMessage {
    /// An incoming JSON-RPC request or notification.
    Request(RpcRequest),
    /// An incoming JSON-RPC response (from the client in callback protocol).
    Response(RpcResponse),
}

impl RpcResponse {
    pub(crate) fn from_inbound_parts(
        jsonrpc: String,
        id: Option<RpcId>,
        result: Option<Box<RawValue>>,
        error: Option<RpcError>,
    ) -> Self {
        Self {
            jsonrpc,
            id,
            result,
            error,
            outbound_permit: None,
        }
    }

    pub(crate) fn from_error(id: Option<RpcId>, error: RpcError) -> Self {
        let RpcError {
            code,
            message,
            data,
        } = error;
        match data {
            Some(data) => Self::error_with_data(id, code, message, data),
            None => Self::error(id, code, message),
        }
    }

    /// Construct a success response with the given result.
    ///
    /// If the result payload fails to serialize, this does NOT fabricate a
    /// success-with-null reply: it surfaces the typed serialization fault as a
    /// JSON-RPC internal error on the same id, preserving the contract that a
    /// request gets either a result or an error, never a fabricated success.
    pub fn success(id: Option<RpcId>, result: impl Serialize) -> Self {
        Self::success_with_admission(
            bounded_outbound_id(id),
            result,
            RpcOutboundAdmission::production(),
        )
    }

    pub(crate) fn success_with_admission(
        id: Option<RpcId>,
        result: impl Serialize,
        admission: RpcOutboundAdmission,
    ) -> Self {
        let id = bounded_outbound_id(id);
        let envelope_bytes = response_envelope_bytes(id.as_ref());
        let max_result_bytes = RPC_OUTBOUND_MAX_MESSAGE_BYTES.saturating_sub(envelope_bytes);
        match admission.admit_json(&result, max_result_bytes, envelope_bytes) {
            Ok((raw, permit)) => Self {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(raw),
                error: None,
                outbound_permit: Some(permit),
            },
            Err(error @ RpcOutboundAdmissionError::TooLarge { .. })
            | Err(error @ RpcOutboundAdmissionError::Backpressured { .. }) => Self::error(
                id,
                crate::error::BUDGET_EXHAUSTED,
                format!("RPC result rejected by outbound admission: {error}"),
            ),
            // Serialization failures are server faults, never fabricated
            // success-with-null envelopes.
            Err(error) => Self::error(
                id,
                crate::error::INTERNAL_ERROR,
                format!("failed to serialize RPC result: {error}"),
            ),
        }
    }

    /// Construct an error response.
    pub fn error(id: Option<RpcId>, code: i32, message: impl Into<String>) -> Self {
        let id = bounded_outbound_id(id);
        let message = truncate_utf8(message.into(), RPC_OUTBOUND_MAX_ERROR_MESSAGE_BYTES);
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message,
                data: None,
            }),
            outbound_permit: None,
        }
    }

    /// Construct an error response with additional data.
    pub fn error_with_data(
        id: Option<RpcId>,
        code: i32,
        message: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        let id = bounded_outbound_id(id);
        let message = truncate_utf8(message.into(), RPC_OUTBOUND_MAX_ERROR_MESSAGE_BYTES);
        let data_bytes = match bounded_json_size(&data, RPC_OUTBOUND_MAX_ERROR_DATA_BYTES) {
            Ok(bytes) => bytes,
            Err(error) => {
                return Self::error(
                    id,
                    code,
                    format!("{message} (bounded error data omitted: {error})"),
                );
            }
        };
        let wire_bytes = data_bytes
            .saturating_add(response_envelope_bytes(id.as_ref()))
            .saturating_add(message.len());
        match RpcOutboundAdmission::production().try_admit(wire_bytes) {
            Ok(permit) => Self {
                jsonrpc: "2.0".to_string(),
                id,
                result: None,
                error: Some(RpcError {
                    code,
                    message,
                    data: Some(data),
                }),
                outbound_permit: Some(permit),
            },
            Err(error) => Self::error(
                id,
                code,
                format!("{message} (bounded error data omitted: {error})"),
            ),
        }
    }

    /// Estimated bytes covered by the process-owned outbound reservation.
    pub(crate) fn admitted_wire_bytes(&self) -> usize {
        self.outbound_permit.as_ref().map_or_else(
            || serde_json::to_vec(self).map_or(0, |bytes| bytes.len()),
            |permit| permit.wire_bytes(),
        )
    }
}

impl RpcNotification {
    /// Construct a new notification.
    pub fn new(
        method: impl Into<String>,
        params: serde_json::Value,
    ) -> Result<Self, RpcOutboundAdmissionError> {
        Self::try_new(method, &params)
    }

    /// Construct an admitted notification directly from a serializable
    /// projection. Sizing is allocation-free and precedes the RawValue retained
    /// by the queue.
    pub(crate) fn try_new(
        method: impl Into<String>,
        params: &(impl Serialize + ?Sized),
    ) -> Result<Self, RpcOutboundAdmissionError> {
        Self::try_new_with_admission(method, params, RpcOutboundAdmission::production())
    }

    pub(crate) fn try_new_with_admission(
        method: impl Into<String>,
        params: &(impl Serialize + ?Sized),
        admission: RpcOutboundAdmission,
    ) -> Result<Self, RpcOutboundAdmissionError> {
        let method = method.into();
        if method.len() > RPC_OUTBOUND_MAX_METHOD_BYTES {
            return Err(RpcOutboundAdmissionError::TooLarge {
                max_bytes: RPC_OUTBOUND_MAX_METHOD_BYTES,
                actual_bytes: method.len(),
            });
        }
        let envelope_bytes = RPC_OUTBOUND_ENVELOPE_SLACK_BYTES.saturating_add(method.len());
        let max_params_bytes = RPC_OUTBOUND_MAX_NOTIFICATION_BYTES.saturating_sub(envelope_bytes);
        let (params, permit) = admission.admit_json(params, max_params_bytes, envelope_bytes)?;
        Ok(Self {
            jsonrpc: "2.0".to_string(),
            method,
            params,
            outbound_permit: Some(permit),
        })
    }

    pub(crate) fn admitted_wire_bytes(&self) -> usize {
        self.outbound_permit.as_ref().map_or_else(
            || serde_json::to_vec(self).map_or(0, |bytes| bytes.len()),
            |permit| permit.wire_bytes(),
        )
    }

    pub fn params_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_str(self.params.get())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::error;

    #[test]
    fn request_roundtrip_numeric_id() {
        let json = r#"{"jsonrpc":"2.0","id":42,"method":"session/create","params":{"model":"claude-opus-4-8"}}"#;
        let req: RpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, Some(RpcId::Num(42)));
        assert_eq!(req.method, "session/create");
        assert!(req.params.is_some());

        // Roundtrip
        let serialized = serde_json::to_string(&req).unwrap();
        let req2: RpcRequest = serde_json::from_str(&serialized).unwrap();
        assert_eq!(req2.id, Some(RpcId::Num(42)));
        assert_eq!(req2.method, "session/create");
    }

    #[test]
    fn request_roundtrip_string_id() {
        let json = r#"{"jsonrpc":"2.0","id":"abc-123","method":"turn/submit","params":{}}"#;
        let req: RpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, Some(RpcId::Str("abc-123".to_string())));

        let serialized = serde_json::to_string(&req).unwrap();
        let req2: RpcRequest = serde_json::from_str(&serialized).unwrap();
        assert_eq!(req2.id, Some(RpcId::Str("abc-123".to_string())));
    }

    #[test]
    fn has_supported_version_accepts_2_0_only() {
        let ok = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: "session/list".to_string(),
            params: None,
        };
        assert!(ok.has_supported_version());

        let wrong = RpcRequest {
            jsonrpc: "1.0".to_string(),
            ..ok.clone()
        };
        assert!(!wrong.has_supported_version());

        let missing = RpcRequest {
            jsonrpc: String::new(),
            ..ok
        };
        assert!(!missing.has_supported_version());
    }

    #[test]
    fn notification_serialization_no_id() {
        let json = r#"{"jsonrpc":"2.0","method":"cancel"}"#;
        let req: RpcRequest = serde_json::from_str(json).unwrap();
        assert!(req.is_notification());
        assert!(req.id.is_none());

        // When serialized, "id" key should not appear
        let serialized = serde_json::to_string(&req).unwrap();
        assert!(!serialized.contains("\"id\""));
    }

    #[test]
    fn success_response_with_result() {
        let resp = RpcResponse::success(Some(RpcId::Num(1)), serde_json::json!({"status": "ok"}));
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(RpcId::Num(1)));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());

        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(serialized.contains("\"result\""));
        assert!(!serialized.contains("\"error\""));

        // Verify result content
        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed["result"]["status"], "ok");
    }

    #[test]
    fn success_serialization_failure_yields_internal_error() {
        // A payload whose Serialize impl always fails must NOT be laundered
        // into a success-with-null envelope; it must become an error envelope.
        struct AlwaysFails;
        impl Serialize for AlwaysFails {
            fn serialize<S: serde::Serializer>(&self, _s: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("boom"))
            }
        }

        let resp = RpcResponse::success(Some(RpcId::Num(1)), AlwaysFails);
        assert_eq!(resp.id, Some(RpcId::Num(1)));
        assert!(resp.result.is_none());
        let err = resp.error.as_ref().unwrap();
        assert_eq!(err.code, error::INTERNAL_ERROR);

        // On the wire it is an error envelope, never a fabricated success.
        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(serialized.contains("\"error\""));
        assert!(!serialized.contains("\"result\""));
    }

    #[test]
    fn error_response_with_code_and_message() {
        let resp = RpcResponse::error(
            Some(RpcId::Num(5)),
            error::METHOD_NOT_FOUND,
            "Method not found",
        );
        assert_eq!(resp.id, Some(RpcId::Num(5)));
        assert!(resp.result.is_none());
        let err = resp.error.as_ref().unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
        assert!(err.data.is_none());

        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(!serialized.contains("\"result\""));
        assert!(serialized.contains("\"error\""));
    }

    #[test]
    fn error_response_without_data_omits_data_field() {
        let resp = RpcResponse::error(
            Some(RpcId::Num(1)),
            error::INTERNAL_ERROR,
            "something broke",
        );
        let serialized = serde_json::to_string(&resp).unwrap();
        // "data" key should not appear when data is None
        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert!(parsed["error"].get("data").is_none());
    }

    #[test]
    fn error_response_with_data() {
        let data = serde_json::json!({"detail": "missing field"});
        let resp = RpcResponse::error_with_data(
            Some(RpcId::Num(2)),
            error::INVALID_PARAMS,
            "Invalid params",
            data.clone(),
        );
        let err = resp.error.as_ref().unwrap();
        assert_eq!(err.data, Some(data));

        let serialized = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed["error"]["data"]["detail"], "missing field");
    }

    #[test]
    fn rpc_id_deserializes_number() {
        let id: RpcId = serde_json::from_str("42").unwrap();
        assert_eq!(id, RpcId::Num(42));
    }

    #[test]
    fn rpc_id_deserializes_string() {
        let id: RpcId = serde_json::from_str(r#""req-1""#).unwrap();
        assert_eq!(id, RpcId::Str("req-1".to_string()));
    }

    #[test]
    fn request_with_no_params() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"session/list"}"#;
        let req: RpcRequest = serde_json::from_str(json).unwrap();
        assert!(req.params.is_none());
        assert!(!req.is_notification());

        // Roundtrip: params should not appear in serialized form
        let serialized = serde_json::to_string(&req).unwrap();
        assert!(!serialized.contains("\"params\""));
    }

    #[test]
    fn response_success_helper() {
        let resp = RpcResponse::success(
            Some(RpcId::Str("x".to_string())),
            serde_json::json!({"sessions": []}),
        );
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(RpcId::Str("x".to_string())));
        assert!(resp.error.is_none());

        let serialized = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed["result"]["sessions"], serde_json::json!([]));
    }

    #[test]
    fn notification_constructor() {
        let notif = RpcNotification::new(
            "session/event",
            serde_json::json!({"type": "token", "text": "hello"}),
        )
        .unwrap();
        assert_eq!(notif.jsonrpc, "2.0");
        assert_eq!(notif.method, "session/event");
        let params = notif.params_value().unwrap();
        assert_eq!(params["type"], "token");
        assert_eq!(params["text"], "hello");

        // Roundtrip
        let serialized = serde_json::to_string(&notif).unwrap();
        let notif2: RpcNotification = serde_json::from_str(&serialized).unwrap();
        assert_eq!(notif2.method, "session/event");
    }

    #[test]
    fn outbound_response_permit_survives_clone_and_releases_on_last_drop() {
        let capacity = RPC_OUTBOUND_MIN_CHARGE_BYTES * RPC_OUTBOUND_MEMORY_MULTIPLIER;
        let admission = RpcOutboundAdmission::new(capacity, 1);
        let response = RpcResponse::success_with_admission(
            Some(RpcId::Num(1)),
            serde_json::json!({"ok": true}),
            admission.clone(),
        );
        assert!(response.result.is_some());
        assert_eq!(admission.available_memory_bytes(), 0);
        assert_eq!(admission.available_count(), 0);

        let cloned = response.clone();
        drop(response);
        assert_eq!(admission.available_memory_bytes(), 0);
        assert_eq!(admission.available_count(), 0);

        drop(cloned);
        assert_eq!(admission.available_memory_bytes(), capacity);
        assert_eq!(admission.available_count(), 1);
    }

    #[test]
    fn saturated_outbound_response_becomes_bounded_budget_error() {
        let capacity = RPC_OUTBOUND_MIN_CHARGE_BYTES * RPC_OUTBOUND_MEMORY_MULTIPLIER;
        let admission = RpcOutboundAdmission::new(capacity, 1);
        let held = RpcResponse::success_with_admission(
            Some(RpcId::Num(1)),
            serde_json::json!({"held": true}),
            admission.clone(),
        );
        let rejected = RpcResponse::success_with_admission(
            Some(RpcId::Num(2)),
            serde_json::json!({"queued": true}),
            admission.clone(),
        );

        assert!(rejected.result.is_none());
        assert_eq!(
            rejected.error.as_ref().map(|error| error.code),
            Some(error::BUDGET_EXHAUSTED)
        );
        assert!(serde_json::to_vec(&rejected).unwrap().len() < 8 * 1024);
        assert_eq!(admission.available_count(), 0);
        drop(held);
        assert_eq!(admission.available_count(), 1);
    }

    #[test]
    fn oversized_notification_is_rejected_before_queue_ownership() {
        let admission = RpcOutboundAdmission::new(
            RPC_OUTBOUND_PROCESS_MEMORY_BUDGET_BYTES,
            RPC_OUTBOUND_PROCESS_MAX_INFLIGHT,
        );
        let oversized = "x".repeat(RPC_OUTBOUND_MAX_NOTIFICATION_BYTES);
        let result = RpcNotification::try_new_with_admission(
            "session/event",
            &serde_json::json!({"payload": oversized}),
            admission.clone(),
        );

        assert!(matches!(
            result,
            Err(RpcOutboundAdmissionError::TooLarge { .. })
        ));
        assert_eq!(
            admission.available_memory_bytes(),
            RPC_OUTBOUND_PROCESS_MEMORY_BUDGET_BYTES
        );
        assert_eq!(
            admission.available_count(),
            RPC_OUTBOUND_PROCESS_MAX_INFLIGHT
        );
    }

    #[test]
    fn bounded_control_errors_fit_the_proven_process_queue_budget() {
        // 128 TCP connections + one stdio surface can each be in one inline
        // control write; 256 admitted request tasks and 64 pending callbacks
        // cover every queued error producer. Their sum stays below the explicit
        // 512-envelope control bound even when the data-plane byte pool is full.
        let worst_case = bounded_control_error_bytes(128 + 1, 256, 64)
            .expect("producer caps fit the control bound");
        assert!(worst_case <= RPC_CONTROL_ERROR_WORST_CASE_BYTES);

        let response = RpcResponse::error(
            Some(RpcId::Str("i".repeat(RPC_OUTBOUND_MAX_ID_BYTES + 1))),
            error::BUDGET_EXHAUSTED,
            "e".repeat(RPC_OUTBOUND_MAX_ERROR_MESSAGE_BYTES * 2),
        );
        assert!(response.id.is_none(), "oversized echo ids fail closed");
        assert!(serde_json::to_vec(&response).unwrap().len() <= RPC_CONTROL_ERROR_MAX_WIRE_BYTES);
    }
}
