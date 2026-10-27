//! Wire protocol parsing for the gateway RPC transport.

use serde::de::DeserializeOwned;

use crate::types::{EventEnvelope, UnifiedEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolParseError {
    InvalidJson,
    InvalidSchema,
    UnexpectedEventKind,
    UnexpectedPayloadType,
}

impl std::fmt::Display for ProtocolParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson => write!(f, "invalid JSON"),
            Self::InvalidSchema => write!(f, "invalid schema"),
            Self::UnexpectedEventKind => write!(f, "unexpected event kind"),
            Self::UnexpectedPayloadType => write!(f, "unexpected payload type"),
        }
    }
}

impl std::error::Error for ProtocolParseError {}

pub fn parse_unified_event_line(
    line: &str,
) -> Result<EventEnvelope<UnifiedEvent>, ProtocolParseError> {
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|_| ProtocolParseError::InvalidJson)?;
    serde_json::from_value(value).map_err(|_| ProtocolParseError::InvalidSchema)
}

pub fn parse_module_event_line<T: DeserializeOwned>(
    line: &str,
    expected_event_type: &str,
) -> Result<EventEnvelope<T>, ProtocolParseError> {
    let envelope = parse_unified_event_line(line)?;

    let module_event = match envelope.event {
        UnifiedEvent::Module(module_event) => module_event,
        _ => return Err(ProtocolParseError::UnexpectedEventKind),
    };

    if module_event.event_type != expected_event_type {
        return Err(ProtocolParseError::UnexpectedPayloadType);
    }

    let typed_payload: T = serde_json::from_value(module_event.payload)
        .map_err(|_| ProtocolParseError::UnexpectedPayloadType)?;

    Ok(EventEnvelope {
        event_id: envelope.event_id,
        source: envelope.source,
        timestamp_ms: envelope.timestamp_ms,
        event: typed_payload,
    })
}
