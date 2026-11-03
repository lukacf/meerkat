//! Canonical parts of a rendering.
//!
//! A rendering is presented in canonical parts: consecutive byte ranges on
//! UTF-8 character boundaries of at most a presenter's part size, each with
//! its own digest, concatenating to the rendering. The split is a pure
//! function of the rendering and the part size. It is the durable decision
//! owner's split exactly (`meerkat_core::approval::decision::canonical_parts`
//! where Core ships it), so a presentation record Meta builds from these
//! parts is the record the decision owner checks for completeness. For a
//! channel with a limit in characters, a part size at or below the limit in
//! bytes is always safe: a part has no more characters than bytes.

use std::num::NonZeroU32;
use std::ops::Range;

use serde::{Deserialize, Serialize};

use super::digest::Sha256Digest;

/// One canonical part of a rendering.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
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
pub fn part_ranges(rendering: &str, max_part_bytes: Option<NonZeroU32>) -> Vec<Range<usize>> {
    let len = rendering.len();
    let Some(max_part_bytes) = max_part_bytes else {
        return std::iter::once(0..len).collect();
    };
    let max = usize::try_from(max_part_bytes.get()).unwrap_or(usize::MAX);
    if len <= max {
        return std::iter::once(0..len).collect();
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
/// (`None`: one part).
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

/// Whether `parts_shown` are exactly the canonical parts of `rendering` for
/// the part size `max_part_bytes`: a presentation that showed every part.
pub fn shows_every_part(
    rendering: &str,
    max_part_bytes: Option<NonZeroU32>,
    parts_shown: &[CanonicalPart],
) -> bool {
    parts_shown == canonical_parts(rendering, max_part_bytes).as_slice()
}
