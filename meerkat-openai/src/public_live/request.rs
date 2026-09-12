use meerkat_core::live_execution::evidence::{LIVE_REQUEST_TEXT_MAX_BYTES, LiveRequestText};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

pub const INVOKE_MEERKAT: &str = "invoke_meerkat";
const MAX_ENCODED_ARGUMENT_BYTES: usize = LIVE_REQUEST_TEXT_MAX_BYTES * 6 + 128;

/// The only public managed function input. The content grants no execution,
/// path, tool, model, or credential authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvokeMeerkatRequest {
    pub request: LiveRequestText,
}

impl InvokeMeerkatRequest {
    pub fn decode(name: &str, arguments: &RawValue) -> Result<Self, InvokeMeerkatDecodeError> {
        Self::decode_arguments(name, arguments.get())
    }

    pub fn decode_arguments(name: &str, arguments: &str) -> Result<Self, InvokeMeerkatDecodeError> {
        if name != INVOKE_MEERKAT {
            return Err(InvokeMeerkatDecodeError::UnknownFunction);
        }
        if arguments.len() > MAX_ENCODED_ARGUMENT_BYTES {
            return Err(InvokeMeerkatDecodeError::EncodedArgumentsTooLarge);
        }
        serde_json::from_str(arguments).map_err(|_| InvokeMeerkatDecodeError::InvalidArguments)
    }

    pub fn parameters_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "request": {"type": "string", "minLength": 1}
            },
            "required": ["request"],
            "additionalProperties": false
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvokeMeerkatDecodeError {
    #[error("unknown public Live managed function")]
    UnknownFunction,
    #[error("public Live function arguments exceed the encoded byte bound")]
    EncodedArgumentsTooLarge,
    #[error("public Live function arguments violate the exact request contract")]
    InvalidArguments,
}
