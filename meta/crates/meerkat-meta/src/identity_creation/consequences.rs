//! Host consequence lines: a host's own warnings about a request, in the
//! canonical rendering.
//!
//! A host states what a request means in its domain (a peer of a sensitive
//! kind, a tool that reads private content) as consequence lines. They are
//! rendered in their own section of `render_v1`, so the subject digest
//! covers them and every complete presentation shows them. A host supplies
//! them in one of two forms:
//!
//! - an embedded host implements `GatewayHostComposition::identity_consequences`;
//! - a gateway host declares the host callback `identity.consequences`
//!   (`CONSEQUENCES_CALLBACK`) and answers it with `{"lines": [..]}`.
//!
//! Either form fails closed. When the host cannot answer (an error, a
//! malformed or late answer), the request is refused
//! `consequences_unavailable` (retryable) and nothing is rendered or held.
//! The lines must be deterministic: the same request gives the same lines.
//! At execution they are computed again; lines that differ change the
//! rendering, and the executor refuses the operation `subject_widened` and
//! closes the decision.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::request::IdentityRequest;

/// The host callback a gateway host declares to supply consequence lines.
pub const CONSEQUENCES_CALLBACK: &str = "identity.consequences";

const FORMAT_CONSEQUENCES: &str = "meerkat.identity.consequences.v1";

/// One host consequence line, in the host's words.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConsequenceLine(String);

impl ConsequenceLine {
    /// A line of `text`.
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The line's text.
    pub fn text(&self) -> &str {
        &self.0
    }
}

/// Why a host could not produce its consequence lines. The request is
/// refused `consequences_unavailable`; the reason is for the host's logs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HookRefusal {
    /// Why, in the host's words.
    pub reason: String,
}

impl HookRefusal {
    /// A refusal for `reason`.
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl fmt::Display for HookRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "consequence lines unavailable: {}", self.reason)
    }
}

impl std::error::Error for HookRefusal {}

/// A source of host consequence lines for a normalized request.
pub trait ConsequenceHook {
    /// The host's lines for `request`, in any order. The same request must
    /// give the same lines.
    fn consequences(&self, request: &IdentityRequest) -> Result<Vec<ConsequenceLine>, HookRefusal>;
}

impl<F> ConsequenceHook for F
where
    F: Fn(&IdentityRequest) -> Result<Vec<ConsequenceLine>, HookRefusal>,
{
    fn consequences(&self, request: &IdentityRequest) -> Result<Vec<ConsequenceLine>, HookRefusal> {
        self(request)
    }
}

/// The parameters of the `identity.consequences` callback for `request`:
/// `{"format", "kind", "request"}`, where `kind` is the decision kind
/// (`identity.create`, `identity.update` or `identity.remove`) and
/// `request` the normalized request.
pub fn consequences_callback_params(request: &IdentityRequest) -> Result<Value, serde_json::Error> {
    Ok(json!({
        "format": FORMAT_CONSEQUENCES,
        "kind": request.decision_kind(),
        "request": serde_json::to_value(request)?,
    }))
}

/// Read the `identity.consequences` callback's result, `{"lines": [..]}`
/// with each line a string. Anything else is a hook refusal.
pub fn parse_consequences_callback_result(
    result: &Value,
) -> Result<Vec<ConsequenceLine>, HookRefusal> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Answer {
        lines: Vec<String>,
    }
    let answer: Answer = serde_json::from_value(result.clone()).map_err(|error| {
        HookRefusal::new(format!("malformed {CONSEQUENCES_CALLBACK} result: {error}"))
    })?;
    Ok(answer.lines.into_iter().map(ConsequenceLine::new).collect())
}
