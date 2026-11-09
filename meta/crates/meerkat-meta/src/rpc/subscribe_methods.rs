//! Parameter parsing for event subscription RPC methods.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SubscribeParamsError {
    ParamsMustBeObject,
    ScopeMustBeString,
    UnsupportedScope(String),
    LastEventIdMustBeString,
    AgentIdMustBeString,
    Runtime(SubscribeError),
}

impl SubscribeParamsError {
    pub(super) fn message(&self) -> String {
        match self {
            SubscribeParamsError::ParamsMustBeObject => {
                "subscribe params must be a JSON object".to_string()
            }
            SubscribeParamsError::ScopeMustBeString => "scope must be a string".to_string(),
            SubscribeParamsError::UnsupportedScope(scope) => {
                format!("unsupported scope '{scope}' (allowed: mob, agent, interaction)")
            }
            SubscribeParamsError::LastEventIdMustBeString => {
                "last_event_id must be a string".to_string()
            }
            SubscribeParamsError::AgentIdMustBeString => "agent_id must be a string".to_string(),
            SubscribeParamsError::Runtime(SubscribeError::EmptyCheckpoint) => {
                "last_event_id cannot be empty".to_string()
            }
            SubscribeParamsError::Runtime(SubscribeError::UnknownCheckpoint(checkpoint)) => {
                format!("unknown last_event_id '{checkpoint}'")
            }
            SubscribeParamsError::Runtime(SubscribeError::MissingAgentId) => {
                "agent_id is required when scope is 'agent'".to_string()
            }
            SubscribeParamsError::Runtime(SubscribeError::InvalidAgentId) => {
                "agent_id cannot be empty when scope is 'agent'".to_string()
            }
        }
    }
}

pub(super) fn parse_subscribe_request(
    params: &Value,
) -> Result<SubscribeRequest, SubscribeParamsError> {
    if params.is_null() {
        return Ok(SubscribeRequest::default());
    }

    let object = params
        .as_object()
        .ok_or(SubscribeParamsError::ParamsMustBeObject)?;

    let scope = match object.get("scope") {
        None => SubscribeScope::Mob,
        Some(value) => {
            let scope = value
                .as_str()
                .ok_or(SubscribeParamsError::ScopeMustBeString)?;
            match scope {
                "mob" => SubscribeScope::Mob,
                "agent" => SubscribeScope::Agent,
                "interaction" => SubscribeScope::Interaction,
                other => {
                    return Err(SubscribeParamsError::UnsupportedScope(other.to_string()));
                }
            }
        }
    };

    let last_event_id = match object.get("last_event_id") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(SubscribeParamsError::LastEventIdMustBeString)?
                .to_string(),
        ),
    };
    let agent_id = match object.get("agent_id") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(SubscribeParamsError::AgentIdMustBeString)?
                .to_string(),
        ),
    };

    Ok(SubscribeRequest {
        scope,
        last_event_id,
        agent_id,
    })
}
