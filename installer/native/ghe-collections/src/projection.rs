//! Select the pinned GitHub payload from Core's native MCP content projection.
//! This is data decoding only. The real adapter must first receive the matching
//! CommittedCollectionResult and preserve its native target and close evidence.

use super::{ReadCall, Refusal};
use serde_json::Value;

/// A borrowed view of the actual native ContentBlock. Native adapters must map
/// variants directly, not serialize a text projection and parse it back.
pub enum ProjectedBlock<'a> {
    Text(&'a str),
    Structured(&'a str),
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadOrigin {
    Text,
    /// Core can remove text equal to structuredContent and keep typed JSON.
    /// This tag identifies the received block, not proof of its provenance.
    Structured,
}

#[derive(Debug)]
pub struct GithubPayload<'a> {
    pub origin: PayloadOrigin,
    payload: &'a str,
    /// A differently wrapped structured projection can accompany the pinned
    /// text. Retained separately; never merged, concatenated or substituted.
    /// This does not claim semantic equality of their descriptive fields.
    pub structured_companion: Option<&'a str>,
}
impl GithubPayload<'_> {
    pub fn bytes(&self) -> &[u8] {
        self.payload.as_bytes()
    }
}

/// Refuses tool errors and unexpected content. The call must be the program's
/// actual pending call, already matched to the native committed observation.
/// No receipt, verified account, source coverage or authorization is created.
pub fn projected_github_payload<'a>(
    call: &ReadCall,
    is_error: bool,
    blocks: impl IntoIterator<Item = ProjectedBlock<'a>>,
) -> Result<GithubPayload<'a>, Refusal> {
    if is_error {
        return Err(Refusal::UnsupportedShape);
    }
    let wrapped = match call.operation() {
        "issue_read" | "actions_list" | "actions_get" => true,
        "pull_request_read" | "list_pull_requests" | "list_issues" | "list_commits"
        | "get_commit" => false,
        _ => return Err(Refusal::UnsupportedShape),
    };
    let mut text = None;
    let mut structured = None;
    let mut total = 0usize;
    let mut count = 0;
    for block in blocks {
        count += 1;
        if count > 2 {
            return Err(Refusal::UnsupportedShape);
        }
        let value = match block {
            ProjectedBlock::Text(value) => {
                if text.replace(value).is_some() {
                    return Err(Refusal::UnsupportedShape);
                }
                value
            }
            ProjectedBlock::Structured(value) => {
                if structured.replace(value).is_some() {
                    return Err(Refusal::UnsupportedShape);
                }
                value
            }
            ProjectedBlock::Unsupported => return Err(Refusal::UnsupportedShape),
        };
        total = total
            .checked_add(value.len())
            .ok_or(Refusal::ResultTooLarge)?;
        if total > 32 * 1024 * 1024 {
            return Err(Refusal::ResultTooLarge);
        }
    }
    // Validate one complete JSON value in each retained block, including the
    // companion. The operation-specific program still validates its own shape.
    if let Some(value) = text {
        let _: Value = serde_json::from_str(value).map_err(|_| Refusal::UnsupportedShape)?;
    }
    if let Some(value) = structured {
        let parsed: Value = serde_json::from_str(value).map_err(|_| Refusal::UnsupportedShape)?;
        if wrapped
            && (call
                .arguments()
                .get("method")
                .and_then(Value::as_str)
                .is_none()
                || parsed.get("method") != call.arguments().get("method"))
        {
            return Err(Refusal::RequestChanged);
        }
    }
    if let Some(payload) = text {
        // These two handlers serialize exactly the same typed result into text
        // and structuredContent. Core coalesces equal JSON. Two blocks here are
        // therefore not the pinned successful shape. List handlers can differ
        // through field projection and zero-value serialization; retain both.
        if structured.is_some() && matches!(call.operation(), "pull_request_read" | "get_commit") {
            return Err(Refusal::UnsupportedShape);
        }
        return Ok(GithubPayload {
            origin: PayloadOrigin::Text,
            payload,
            structured_companion: structured,
        });
    }
    // The pinned issue and Actions operations always tag their structured wrapper
    // with method, unlike their text. Core cannot coalesce those into one block.
    // Accepting the wrapper alone would invent a missing textual observation.
    if wrapped {
        return Err(Refusal::UnsupportedShape);
    }
    let payload = structured.ok_or(Refusal::UnsupportedShape)?;
    Ok(GithubPayload {
        origin: PayloadOrigin::Structured,
        payload,
        structured_companion: None,
    })
}
