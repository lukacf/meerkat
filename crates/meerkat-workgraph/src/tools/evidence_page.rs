//! Tool-only evidence pages over the one WorkItem the ordinary get returned.
//! The cursor binds its append position to that item's scope and revision;
//! it is neither read authority nor a persisted historical snapshot.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::read_page::{self, PageRead};
use crate::{WorkEvidenceRef, WorkItem, WorkItemId, WorkNamespace};

use super::{WorkGraphToolError, WorkGraphToolErrorCode, map_error};

pub(super) const MAX_EVIDENCE_LIMIT: usize = 20;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceCursor {
    realm_id: String,
    namespace: WorkNamespace,
    item_id: WorkItemId,
    revision: u64,
    /// The next page contains only original positions below this one.
    before: usize,
}

#[derive(Serialize)]
struct EvidenceEntry<'a> {
    position: usize,
    withdrawn: bool,
    evidence: &'a WorkEvidenceRef,
}

#[derive(Serialize)]
struct EvidencePage<'a> {
    order: &'static str,
    /// Full evidence vector length, including earlier pages and withdrawals.
    total: usize,
    returned: usize,
    limit: usize,
    /// Older positions remain after this page, not whether this is full history.
    truncated: bool,
    next_cursor: Option<String>,
    entries: Vec<EvidenceEntry<'a>>,
}

fn invalid(message: &str) -> WorkGraphToolError {
    WorkGraphToolError::new(WorkGraphToolErrorCode::InvalidArguments, message)
}

/// Called only after the existing get has checked scope and read the item.
/// No domain request or stored object gains presentation options.
pub(super) fn project(item: WorkItem, arguments: &Value) -> Result<Value, WorkGraphToolError> {
    let Some(limit) = arguments.get("evidence_limit") else {
        if arguments.get("evidence_cursor").is_some() {
            return Err(invalid("an evidence cursor requires evidence_limit"));
        }
        return Ok(json!({ "item": item }));
    };
    if arguments.get("cursor").is_some() {
        return Err(invalid(
            "use evidence_cursor with evidence_limit; cursor does not continue evidence pages",
        ));
    }
    let limit = limit
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| (1..=MAX_EVIDENCE_LIMIT).contains(value))
        .ok_or_else(|| invalid("evidence_limit must be an integer from 1 through 20"))?;

    let total = item.evidence_refs.len();
    let before = match arguments.get("evidence_cursor") {
        None => total,
        Some(raw) => {
            let raw = raw
                .as_str()
                .ok_or_else(|| invalid("an evidence cursor must be a string"))?;
            let cursor = read_page::decode_cursor::<EvidenceCursor>(raw, PageRead::Evidence)
                .map_err(map_error)?;
            let key = cursor.key;
            if cursor.anchor.is_some()
                || key.realm_id != item.realm_id
                || key.namespace != item.namespace
                || key.item_id != item.id
            {
                return Err(invalid(
                    "evidence cursor does not belong to this item and scope",
                ));
            }
            if key.revision != item.revision {
                return Err(WorkGraphToolError::new(
                    WorkGraphToolErrorCode::Conflict,
                    "evidence cursor revision is stale; restart the read without a cursor",
                ));
            }
            if key.before > total {
                return Err(invalid(
                    "evidence cursor position is outside this item's history",
                ));
            }
            key.before
        }
    };

    let start = before.saturating_sub(limit);
    let entries = (start..before)
        .rev()
        .map(|position| EvidenceEntry {
            position,
            // Read against the full vector, including withdrawals that a
            // newer page already returned and later reuse of the same id.
            withdrawn: item.is_evidence_withdrawn(position),
            evidence: &item.evidence_refs[position],
        })
        .collect::<Vec<_>>();
    let next_cursor = if start > 0 {
        Some(
            read_page::encode_cursor(
                PageRead::Evidence,
                &EvidenceCursor {
                    realm_id: item.realm_id.clone(),
                    namespace: item.namespace.clone(),
                    item_id: item.id.clone(),
                    revision: item.revision,
                    before: start,
                },
                None,
            )
            .map_err(map_error)?,
        )
    } else {
        None
    };
    let page = EvidencePage {
        order: "append_position_desc",
        total,
        returned: entries.len(),
        limit,
        truncated: next_cursor.is_some(),
        next_cursor,
        entries,
    };
    let mut item_value = serde_json::to_value(&item).map_err(|error| {
        WorkGraphToolError::new(
            WorkGraphToolErrorCode::InternalError,
            format!("WorkGraph item is not encodable: {error}"),
        )
    })?;
    let Value::Object(fields) = &mut item_value else {
        return Err(WorkGraphToolError::new(
            WorkGraphToolErrorCode::InternalError,
            "WorkGraph item did not encode as an object",
        ));
    };
    // Keep an incomplete page distinct from the authoritative stored vector.
    fields.remove("evidence_refs");
    Ok(json!({ "item": item_value, "evidence_page": page }))
}
