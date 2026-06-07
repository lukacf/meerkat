//! Typed wire contract for event-stream read terminal status.
//!
//! A stream read can resolve into exactly one of three terminal shapes: a
//! delivered event, an expired read timeout, or a closed (exhausted) stream.
//! Surfaces (MCP server, future SDK consumers) must serialize this typed
//! contract rather than hand-rolling `status: "event" | "timeout" | "closed"`
//! string conventions, so terminal/read state is a single typed authority.

use serde::{Deserialize, Serialize};

/// Opaque pass-through helper mirroring the `SessionExternalEventEnvelope`
/// allow-list: the delivered event body rides as `Box<RawValue>` because the
/// domain `EventEnvelope<AgentEvent>` lives in `meerkat-core` and is never
/// pattern-matched at the wire layer.
fn deserialize_raw_json_box<'de, D>(
    deserializer: D,
) -> Result<Box<serde_json::value::RawValue>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    serde_json::value::RawValue::from_string(value.to_string()).map_err(serde::de::Error::custom)
}

/// Typed terminal status for a single event-stream read.
///
/// Internally tagged on `status` to match the canonical read response shape:
/// `{ "status": "event", "event": { .. } }`, `{ "status": "timeout" }`, or
/// `{ "status": "closed" }`. The transport-level stream identifier is carried
/// by the enclosing response, not by this status (one fact, one owner).
///
/// Not `PartialEq`: the `Event.event` body rides as `Box<RawValue>` — opaque
/// caller-observed JSON (a serialized core `EventEnvelope<AgentEvent>`) that is
/// never pattern-matched at this layer. Allow-listed per `dogma-blind-spots`
/// §7, identical to `SessionExternalEventEnvelope`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StreamReadStatus {
    /// An event was delivered from the stream before the read deadline.
    Event {
        #[serde(deserialize_with = "deserialize_raw_json_box")]
        #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
        event: Box<serde_json::value::RawValue>,
    },
    /// The read deadline expired before any event was available. The stream
    /// remains open and may be read again.
    Timeout,
    /// The stream was exhausted (closed). No further events will arrive.
    Closed,
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn event_status_roundtrip_carries_opaque_payload() {
        let status = StreamReadStatus::Event {
            event: serde_json::value::RawValue::from_string(
                serde_json::json!({"kind": "delta", "text": "hi"}).to_string(),
            )
            .expect("raw value"),
        };
        let json = serde_json::to_value(&status).expect("serialize");
        assert_eq!(json["status"], "event");
        assert_eq!(json["event"]["kind"], "delta");

        let parsed: StreamReadStatus = serde_json::from_value(json).expect("deserialize");
        match parsed {
            StreamReadStatus::Event { event } => {
                let value: serde_json::Value =
                    serde_json::from_str(event.get()).expect("parse opaque event");
                assert_eq!(value["text"], "hi");
            }
            other => panic!("expected Event, got {other:?}"),
        }
    }

    #[test]
    fn timeout_status_roundtrip() {
        let status = StreamReadStatus::Timeout;
        let json = serde_json::to_value(&status).expect("serialize");
        assert_eq!(json, serde_json::json!({"status": "timeout"}));
        let parsed: StreamReadStatus = serde_json::from_value(json).expect("deserialize");
        assert!(matches!(parsed, StreamReadStatus::Timeout));
    }

    #[test]
    fn closed_status_roundtrip() {
        let status = StreamReadStatus::Closed;
        let json = serde_json::to_value(&status).expect("serialize");
        assert_eq!(json, serde_json::json!({"status": "closed"}));
        let parsed: StreamReadStatus = serde_json::from_value(json).expect("deserialize");
        assert!(matches!(parsed, StreamReadStatus::Closed));
    }

    #[test]
    fn unknown_status_is_rejected() {
        let json = serde_json::json!({"status": "drained"});
        let err = serde_json::from_value::<StreamReadStatus>(json)
            .expect_err("unknown status must fail closed");
        assert!(err.to_string().contains("unknown variant"));
    }
}
