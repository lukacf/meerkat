//! The id of one identity change, and the one way it is made.
//!
//! Core keys a change by an operation id in its mob: the same id and request
//! replay the committed change, another request under the same id is
//! refused `operation_reused`. A caller supplies a request id; it becomes
//! Core's operation id only through [`OperationId::for_request`], which
//! namespaces it by the requester (its kind and its id), so no requester's
//! id replays, collides with or probes another requester's operation. A
//! tool call that gives no request id gets one derived before any side
//! effect ([`OperationId::for_tool_call`]). Nothing else derives an
//! operation id.

use std::fmt;

use serde::Serialize;
use serde_json::json;

use super::digest::canonical_digest;
use super::refusal::{IdentityRefusal, IdentityRefusalKind, RequestProblem};
use super::request::Requester;

/// The most bytes a caller's request id may take.
pub const REQUEST_ID_MAX_BYTES: usize = 256;

const FORMAT_REQUESTER: &str = "meerkat.identity.operation_requester.v1";
const FORMAT_TOOL_CALL: &str = "meerkat.identity.tool_call_request.v1";
/// The hex digits of the requester's digest an operation id carries.
const REQUESTER_DIGEST_HEX: usize = 32;
const SHA256_PREFIX: &str = "sha256:";

/// The id of one identity change in its mob: Core's operation id. Its text
/// is `<requester kind>.<requester digest>.<request id>`; a caller that
/// reads an operation back names it by the same requester and request id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct OperationId(String);

/// Where a tool call that gave no request id ran: what its derived request
/// id is made of. Every member is taken from the dispatch that carries the
/// call, never from its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolCallOrigin<'a> {
    /// The mob the call changes.
    pub mob_id: &'a str,
    /// The session the call ran in.
    pub session_id: &'a str,
    /// The run of that session the call belongs to.
    pub run_id: &'a str,
    /// The round of that run the call belongs to: one answer of the
    /// agent's model. A model numbers its tool calls anew in each answer,
    /// so a call id is unique within one round only.
    pub round: &'a str,
    /// The tool call's id within its round.
    pub tool_call_id: &'a str,
}

impl<'a> ToolCallOrigin<'a> {
    /// The tool call `tool_call_id` of the round `round` of the run
    /// `run_id` of the session `session_id`, changing the mob `mob_id`.
    pub fn new(
        mob_id: &'a str,
        session_id: &'a str,
        run_id: &'a str,
        round: &'a str,
        tool_call_id: &'a str,
    ) -> Self {
        Self {
            mob_id,
            session_id,
            run_id,
            round,
            tool_call_id,
        }
    }
}

impl OperationId {
    /// The operation id of `requester`'s request `request_id`. Refused
    /// `invalid_request` at `request_id` for an id that is empty, has a
    /// control character, or is past [`REQUEST_ID_MAX_BYTES`].
    pub fn for_request(requester: &Requester, request_id: &str) -> Result<Self, IdentityRefusal> {
        if request_id.trim().is_empty()
            || request_id.len() > REQUEST_ID_MAX_BYTES
            || request_id.chars().any(char::is_control)
        {
            return Err(IdentityRefusal::new(IdentityRefusalKind::InvalidRequest {
                problem: RequestProblem::Malformed,
            })
            .at("request_id")
            .with_detail(format!(
                "A request id is 1 to {REQUEST_ID_MAX_BYTES} bytes of text without control \
                 characters."
            )));
        }
        Ok(Self::namespaced(requester, request_id))
    }

    /// The operation id of a tool call by `requester` that gave no request
    /// id, derived from where the call ran (its mob, session, run, round
    /// and call id): the same call derives the same id, so its retry
    /// replays, and the same call id in another round of the run is
    /// another operation.
    pub fn for_tool_call(requester: &Requester, origin: &ToolCallOrigin<'_>) -> Self {
        let derived = canonical_digest(&json!({
            "format": FORMAT_TOOL_CALL,
            "mob_id": origin.mob_id,
            "session_id": origin.session_id,
            "run_id": origin.run_id,
            "round": origin.round,
            "tool_call_id": origin.tool_call_id,
        }));
        let hex = derived
            .as_str()
            .strip_prefix(SHA256_PREFIX)
            .unwrap_or(derived.as_str());
        Self::namespaced(requester, &format!("call-{hex}"))
    }

    fn namespaced(requester: &Requester, request_id: &str) -> Self {
        let (kind, id) = match requester {
            Requester::Agent { identity } => ("agent", identity.as_str()),
            Requester::Host { surface } => ("host", surface.as_str()),
            Requester::Human { principal } => ("human", principal.as_str()),
        };
        let digest = canonical_digest(&json!({
            "format": FORMAT_REQUESTER,
            "kind": kind,
            "id": id,
        }));
        let hex = digest
            .as_str()
            .strip_prefix(SHA256_PREFIX)
            .unwrap_or(digest.as_str());
        let short = hex.get(..REQUESTER_DIGEST_HEX).unwrap_or(hex);
        Self(format!("{kind}.{short}.{request_id}"))
    }

    /// An operation id read back from where one was recorded (a receipt, a
    /// held decision, a list's open operations): `None` unless `recorded`
    /// has the form this type derives. It derives nothing: a change that
    /// waited for its decision is committed under the id it was held with,
    /// and an operation is read back by the id an outcome named.
    pub fn from_recorded(recorded: &str) -> Option<Self> {
        let mut parts = recorded.splitn(3, '.');
        let (kind, digest, request_id) = (parts.next()?, parts.next()?, parts.next()?);
        let well_formed = matches!(kind, "agent" | "host" | "human")
            && digest.len() == REQUESTER_DIGEST_HEX
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && !request_id.trim().is_empty()
            && request_id.len() <= REQUEST_ID_MAX_BYTES
            && !request_id.chars().any(char::is_control);
        well_formed.then(|| Self(recorded.to_owned()))
    }

    /// The id's text, as Core's receipt records it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
