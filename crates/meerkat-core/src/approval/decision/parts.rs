//! Canonical parts of a rendering, and durable presentation records.
//!
//! A rendering is presented in canonical parts: consecutive byte ranges on
//! UTF-8 character boundaries of at most a presenter's part size, each with
//! its own digest. The split is a pure function of the rendering and the
//! part size, so a presentation is complete exactly when the parts it shows
//! equal the split for its own part size. A presenter's escaping and channel
//! splitting are never digest inputs.

use std::num::NonZeroU32;
use std::ops::Range;

use serde::{Deserialize, Serialize};

use super::{ApproverId, DecisionPrincipalKey, DecisionReference, DecisionRendering, Sha256Digest};

/// One canonical part of a rendering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalPart {
    /// The part's position, from 0.
    pub index: u32,
    /// How many parts the split has.
    pub count: u32,
    /// The digest of the part's bytes.
    pub part_digest: Sha256Digest,
}

/// The byte ranges of the canonical split of `rendering`: one range for the
/// whole rendering when `max_part_bytes` is `None` or the rendering fits in
/// one part, else consecutive ranges of at most `max_part_bytes` bytes that
/// end on character boundaries. A character longer than the part size is a
/// part of its own.
pub(crate) fn part_ranges(
    rendering: &str,
    max_part_bytes: Option<NonZeroU32>,
) -> Vec<Range<usize>> {
    let len = rendering.len();
    let Some(max_part_bytes) = max_part_bytes else {
        return vec![0..len];
    };
    let max = usize::try_from(max_part_bytes.get()).unwrap_or(usize::MAX);
    if len <= max {
        return vec![0..len];
    }
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < len {
        let mut end = start.saturating_add(max).min(len);
        while end > start && !rendering.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = start + 1;
            while end < len && !rendering.is_char_boundary(end) {
                end += 1;
            }
        }
        ranges.push(start..end);
        start = end;
    }
    ranges
}

/// The canonical parts of `rendering` for the part size `max_part_bytes`
/// (`None`: one part). Deterministic: the same rendering and part size give
/// the same parts.
#[must_use]
pub fn canonical_parts(rendering: &str, max_part_bytes: Option<NonZeroU32>) -> Vec<CanonicalPart> {
    let ranges = part_ranges(rendering, max_part_bytes);
    let count = u32::try_from(ranges.len()).unwrap_or(u32::MAX);
    ranges
        .into_iter()
        .enumerate()
        .map(|(index, range)| CanonicalPart {
            index: u32::try_from(index).unwrap_or(u32::MAX),
            count,
            part_digest: Sha256Digest::of(rendering.get(range).unwrap_or_default().as_bytes()),
        })
        .collect()
}

/// Who a presentation was shown to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PresentationAudience {
    /// Whoever acts for the decision's approver id.
    Approver { approver_id: ApproverId },
    /// The decision's named reviewer.
    Reviewer { reviewer: DecisionPrincipalKey },
}

/// One durable record of a presentation of a decision's rendering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresentationRecord {
    pub audience: PresentationAudience,
    /// The digest of the rendering that was presented.
    pub subject_digest: Sha256Digest,
    /// The presenter's part size; `None` presents the rendering as one part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_part_bytes: Option<NonZeroU32>,
    /// The parts the audience was shown.
    pub parts_shown: Vec<CanonicalPart>,
    /// Where the presentation was shown.
    pub channel_ref: DecisionReference,
    pub message_ref: DecisionReference,
    /// The host's clock reading of the presentation, in UNIX milliseconds.
    pub at_ms: u64,
}

impl PresentationRecord {
    /// The record of a presentation that showed every canonical part of
    /// `rendering` for the part size `max_part_bytes`.
    #[must_use]
    pub fn complete(
        audience: PresentationAudience,
        rendering: &DecisionRendering,
        max_part_bytes: Option<NonZeroU32>,
        channel_ref: DecisionReference,
        message_ref: DecisionReference,
        at_ms: u64,
    ) -> Self {
        Self {
            audience,
            subject_digest: rendering.subject_digest(),
            max_part_bytes,
            parts_shown: canonical_parts(rendering.as_str(), max_part_bytes),
            channel_ref,
            message_ref,
            at_ms,
        }
    }

    /// Whether the parts this record shows are exactly the canonical split
    /// of `rendering` for the record's own part size.
    #[must_use]
    pub fn shows_every_part_of(&self, rendering: &DecisionRendering) -> bool {
        self.parts_shown == canonical_parts(rendering.as_str(), self.max_part_bytes)
    }
}
