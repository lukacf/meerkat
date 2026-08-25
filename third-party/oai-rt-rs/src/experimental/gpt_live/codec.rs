use super::protocol::{
    ClientEvent, CreateCallRequest, ExtraFields, InputTextContent, ServerEvent, UnknownEvent,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("private event is not valid JSON")]
    InvalidJson,
    #[error("private event must be a JSON object")]
    NotAnObject,
    #[error("private event is missing its type discriminant")]
    MissingDiscriminant,
    #[error("private event type discriminant must be a string")]
    InvalidDiscriminant,
    #[error("malformed known private event of kind {kind}")]
    MalformedKnownEvent { kind: &'static str },
    #[error("private event serialization failed")]
    Serialization,
    #[error("private event {scope} extras contain reserved field {field}")]
    ReservedExtraField {
        scope: &'static str,
        field: &'static str,
    },
}

/// Decode one private server event without conflating protocol drift with a
/// malformed event whose discriminant is already known.
///
/// # Errors
///
/// Returns [`CodecError`] when JSON, its discriminant, or a known event body is
/// malformed.
pub fn decode_server_event(input: &str) -> Result<ServerEvent, CodecError> {
    let value: Value = serde_json::from_str(input).map_err(|_| CodecError::InvalidJson)?;
    let object = value.as_object().ok_or(CodecError::NotAnObject)?;
    let kind_value = object.get("type").ok_or(CodecError::MissingDiscriminant)?;
    let kind = kind_value
        .as_str()
        .ok_or(CodecError::InvalidDiscriminant)?
        .to_owned();

    match kind.as_str() {
        "session.started" => decode_known(value, "session.started", ServerEvent::SessionStarted),
        "session.context.appended" => decode_known(
            value,
            "session.context.appended",
            ServerEvent::SessionContextAppended,
        ),
        "input_transcript.added" => decode_known(
            value,
            "input_transcript.added",
            ServerEvent::InputTranscriptAdded,
        ),
        "output_transcript.added" => decode_known(
            value,
            "output_transcript.added",
            ServerEvent::OutputTranscriptAdded,
        ),
        "turn.created" => decode_known(value, "turn.created", ServerEvent::TurnCreated),
        "turn.delta" => decode_known(value, "turn.delta", ServerEvent::TurnDelta),
        "turn.done" => decode_known(value, "turn.done", ServerEvent::TurnDone),
        "delegation.created" => {
            decode_known(value, "delegation.created", ServerEvent::DelegationCreated)
        }
        "delegation.context.appended" => decode_known(
            value,
            "delegation.context.appended",
            ServerEvent::DelegationContextAppended,
        ),
        _ => Ok(ServerEvent::Unknown(UnknownEvent::new(kind, value))),
    }
}

fn decode_known<T>(
    mut value: Value,
    kind: &'static str,
    wrap: impl FnOnce(T) -> ServerEvent,
) -> Result<ServerEvent, CodecError>
where
    T: DeserializeOwned,
{
    value
        .as_object_mut()
        .expect("known event was already checked as an object")
        .remove("type");
    serde_json::from_value(value)
        .map(wrap)
        .map_err(|_| CodecError::MalformedKnownEvent { kind })
}

/// Encode one of the verified private client events.
///
/// # Errors
///
/// Returns [`CodecError`] when serialization fails.
pub fn encode_client_event(event: &ClientEvent) -> Result<String, CodecError> {
    validate_client_event(event)?;
    match event {
        ClientEvent::SessionContextAppend(body) => encode_known("session.context.append", body),
        ClientEvent::DelegationContextAppend(body) => {
            encode_known("delegation.context.append", body)
        }
    }
}

/// Encode a private server event, including an unknown event's complete raw JSON.
///
/// # Errors
///
/// Returns [`CodecError`] when serialization fails.
pub fn encode_server_event(event: &ServerEvent) -> Result<String, CodecError> {
    match event {
        ServerEvent::SessionStarted(body) => encode_known("session.started", body),
        ServerEvent::SessionContextAppended(body) => encode_known("session.context.appended", body),
        ServerEvent::InputTranscriptAdded(body) => encode_known("input_transcript.added", body),
        ServerEvent::OutputTranscriptAdded(body) => encode_known("output_transcript.added", body),
        ServerEvent::TurnCreated(body) => encode_known("turn.created", body),
        ServerEvent::TurnDelta(body) => encode_known("turn.delta", body),
        ServerEvent::TurnDone(body) => encode_known("turn.done", body),
        ServerEvent::DelegationCreated(body) => encode_known("delegation.created", body),
        ServerEvent::DelegationContextAppended(body) => {
            encode_known("delegation.context.appended", body)
        }
        ServerEvent::Unknown(event) => {
            serde_json::to_string(event.raw()).map_err(|_| CodecError::Serialization)
        }
    }
}

fn encode_known(kind: &'static str, body: &impl Serialize) -> Result<String, CodecError> {
    let mut value = serde_json::to_value(body).map_err(|_| CodecError::Serialization)?;
    let object = value.as_object_mut().ok_or(CodecError::NotAnObject)?;
    object.insert("type".to_owned(), Value::String(kind.to_owned()));
    serde_json::to_string(&value).map_err(|_| CodecError::Serialization)
}

pub fn encode_create_call_request(request: &CreateCallRequest) -> Result<Vec<u8>, CodecError> {
    reject_reserved(
        &request.session.extra,
        "call session",
        &["model", "audio", "delegation", "instructions"],
    )?;
    reject_reserved(&request.session.audio.extra, "session audio", &["output"])?;
    reject_reserved(
        &request.session.audio.output.extra,
        "session audio output",
        &["voice"],
    )?;
    if let Some(delegation) = &request.session.delegation {
        reject_reserved(&delegation.extra, "client delegation", &["type"])?;
    }
    serde_json::to_vec(request).map_err(|_| CodecError::Serialization)
}

fn validate_client_event(event: &ClientEvent) -> Result<(), CodecError> {
    match event {
        ClientEvent::SessionContextAppend(body) => {
            reject_reserved(
                &body.extra,
                "session context append",
                &["type", "channel", "content"],
            )?;
            validate_content(&body.content)
        }
        ClientEvent::DelegationContextAppend(body) => {
            reject_reserved(
                &body.extra,
                "delegation context append",
                &["type", "delegation_item_id", "channel", "content"],
            )?;
            validate_content(&body.content)
        }
    }
}

fn validate_content(content: &[InputTextContent]) -> Result<(), CodecError> {
    for item in content {
        reject_reserved(
            &item.extra,
            "input text content",
            &["type", "text", "channel"],
        )?;
    }
    Ok(())
}

fn reject_reserved(
    extra: &ExtraFields,
    scope: &'static str,
    reserved: &[&'static str],
) -> Result<(), CodecError> {
    for &field in reserved {
        if extra.contains_key(field) {
            return Err(CodecError::ReservedExtraField { scope, field });
        }
    }
    Ok(())
}
