//! Keyset pages over the WorkGraph reads.
//!
//! `list` and `snapshot` order items most recently updated first
//! (`updated_at`, then `id`, both descending); `ready` orders them by
//! priority (high first), then `created_at` and `id` ascending. A page that
//! ends early hands back an opaque cursor: the read it continues, the order
//! key of the last row it returned, and the walk's anchor. The continuation
//! returns the rows strictly after that key in the same order, so rows never
//! shift between pages. The anchor is the event ledger's high-water mark when
//! the walk began; every page reports whether the ledger has moved past it.
//!
//! A cursor is not authority: every page is scoped by the namespace grant
//! like the first one, so a forged cursor can only skip rows the caller may
//! read anyway.

use std::cmp::Ordering;

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::WorkGraphError;
use crate::types::{WorkItem, WorkItemId, WorkPriority};

/// The version tag every cursor starts with. A cursor of another version is
/// refused, not reinterpreted.
const CURSOR_PREFIX: &str = "wg1.";

const BASE64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// The read a cursor continues. A cursor of one read is refused by another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PageRead {
    List,
    Ready,
    Snapshot,
}

impl PageRead {
    const fn name(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Ready => "ready",
            Self::Snapshot => "snapshot",
        }
    }
}

/// A row's position in the `list` and `snapshot` order: `[updated_at, id]`.
pub(crate) type UpdatedKey = (DateTime<Utc>, WorkItemId);

/// A row's position in the `ready` order: `[priority, created_at, id]`.
pub(crate) type ReadyKey = (WorkPriority, DateTime<Utc>, WorkItemId);

/// The cursor's body: `op`, the read; `k`, the order key of the last row
/// returned; `a`, the walk's anchor.
#[derive(Serialize, Deserialize)]
struct CursorBody<K> {
    op: PageRead,
    k: K,
    a: Option<i64>,
}

/// A decoded cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageCursor<K> {
    /// The order key of the last row the previous page returned.
    pub(crate) key: K,
    /// The ledger high-water mark when the walk began.
    pub(crate) anchor: Option<i64>,
}

/// Encode the cursor that continues `read` after the row at `key`.
pub(crate) fn encode_cursor<K: Serialize>(
    read: PageRead,
    key: &K,
    anchor: Option<i64>,
) -> Result<String, WorkGraphError> {
    let body = serde_json::to_vec(&CursorBody {
        op: read,
        k: key,
        a: anchor,
    })
    .map_err(|err| WorkGraphError::Store(format!("WorkGraph cursor is not encodable: {err}")))?;
    Ok(format!("{CURSOR_PREFIX}{}", base64url_encode(&body)))
}

/// Decode a cursor `read` handed out. A string that is not a cursor, a cursor
/// of another version, one whose body does not decode, and one of another
/// read are all refused as invalid input.
pub(crate) fn decode_cursor<K: DeserializeOwned>(
    cursor: &str,
    read: PageRead,
) -> Result<PageCursor<K>, WorkGraphError> {
    let refused = || {
        WorkGraphError::InvalidInput(format!(
            "cursor is not a WorkGraph {} cursor: pass back the next_cursor a {} page returned",
            read.name(),
            read.name()
        ))
    };
    let body = cursor
        .strip_prefix(CURSOR_PREFIX)
        .and_then(base64url_decode)
        .ok_or_else(refused)?;
    let body: CursorBody<K> = serde_json::from_slice(&body).map_err(|_| refused())?;
    if body.op != read {
        return Err(refused());
    }
    Ok(PageCursor {
        key: body.k,
        anchor: body.a,
    })
}

/// The `list` and `snapshot` order: `updated_at` descending, then `id`
/// descending, so a cut keeps the most recently updated items. It compares
/// the decoded timestamps, never the order a store returned its rows in.
pub(crate) fn updated_order(left: &WorkItem, right: &WorkItem) -> Ordering {
    updated_key_order((&left.updated_at, &left.id), (&right.updated_at, &right.id))
}

fn updated_key_order(
    left: (&DateTime<Utc>, &WorkItemId),
    right: (&DateTime<Utc>, &WorkItemId),
) -> Ordering {
    right.0.cmp(left.0).then_with(|| right.1.cmp(left.1))
}

pub(crate) fn updated_key(item: &WorkItem) -> UpdatedKey {
    (item.updated_at, item.id.clone())
}

/// Whether `item` comes strictly after `key` in the `list` and `snapshot`
/// order.
pub(crate) fn after_updated_key(item: &WorkItem, key: &UpdatedKey) -> bool {
    updated_key_order((&item.updated_at, &item.id), (&key.0, &key.1)) == Ordering::Greater
}

/// The `ready` order: `priority` descending, then `created_at` and `id`
/// ascending.
pub(crate) fn ready_order(left: &WorkItem, right: &WorkItem) -> Ordering {
    ready_key_order(
        (left.priority, &left.created_at, &left.id),
        (right.priority, &right.created_at, &right.id),
    )
}

fn ready_key_order(
    left: (WorkPriority, &DateTime<Utc>, &WorkItemId),
    right: (WorkPriority, &DateTime<Utc>, &WorkItemId),
) -> Ordering {
    right
        .0
        .cmp(&left.0)
        .then_with(|| left.1.cmp(right.1))
        .then_with(|| left.2.cmp(right.2))
}

pub(crate) fn ready_key(item: &WorkItem) -> ReadyKey {
    (item.priority, item.created_at, item.id.clone())
}

/// Whether `item` comes strictly after `key` in the `ready` order.
pub(crate) fn after_ready_key(item: &WorkItem, key: &ReadyKey) -> bool {
    ready_key_order(
        (item.priority, &item.created_at, &item.id),
        (key.0, &key.1, &key.2),
    ) == Ordering::Greater
}

/// Where a page starts in `rows`, sorted in its read's order: after every row
/// at or before the cursor's position (`is_after` is false for those), or at
/// the first row on a first page.
pub(crate) fn page_start<T>(rows: &[T], is_after: impl Fn(&T) -> bool) -> usize {
    rows.partition_point(|row| !is_after(row))
}

fn base64url_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 6 {
            bits -= 6;
            out.push(char::from(
                BASE64URL_ALPHABET[((buffer >> bits) & 0x3f) as usize],
            ));
        }
        buffer &= (1u32 << bits) - 1;
    }
    if bits > 0 {
        out.push(char::from(
            BASE64URL_ALPHABET[((buffer << (6 - bits)) & 0x3f) as usize],
        ));
    }
    out
}

/// Unpadded base64url back to bytes; `None` for any character outside the
/// alphabet, an impossible length, or non-zero trailing bits.
fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3 + 2);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1u32 << bits) - 1;
        }
    }
    (bits < 6 && buffer == 0).then_some(out)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn base64url_round_trips_and_matches_known_vectors() {
        for (bytes, text) in [
            (&b""[..], ""),
            (&b"M"[..], "TQ"),
            (&b"Ma"[..], "TWE"),
            (&b"Man"[..], "TWFu"),
            (&[0xfb, 0xff][..], "-_8"),
        ] {
            assert_eq!(base64url_encode(bytes), text);
            assert_eq!(base64url_decode(text).as_deref(), Some(bytes));
        }
        let all = (0..=255u8).collect::<Vec<_>>();
        assert_eq!(base64url_decode(&base64url_encode(&all)), Some(all));
    }

    #[test]
    fn base64url_refuses_what_it_never_writes() {
        // Padding, a character outside the alphabet, a lone trailing
        // character, and non-zero trailing bits.
        for text in ["TQ==", "T+Fu", "TWFuT", "TR"] {
            assert_eq!(base64url_decode(text), None, "{text}");
        }
    }

    #[test]
    fn a_cursor_round_trips_for_its_own_read_only() {
        let key: UpdatedKey = (Utc::now(), WorkItemId::new("work_a").unwrap());
        let cursor = encode_cursor(PageRead::List, &key, Some(42)).unwrap();
        assert!(cursor.starts_with(CURSOR_PREFIX));
        let decoded = decode_cursor::<UpdatedKey>(&cursor, PageRead::List).unwrap();
        assert_eq!(decoded.key, key);
        assert_eq!(decoded.anchor, Some(42));
        // The same key shape, another read.
        assert!(decode_cursor::<UpdatedKey>(&cursor, PageRead::Snapshot).is_err());
        // Another key shape.
        assert!(decode_cursor::<ReadyKey>(&cursor, PageRead::Ready).is_err());
        // Another version, and no cursor at all.
        let other_version = cursor.replacen(CURSOR_PREFIX, "wg2.", 1);
        assert!(decode_cursor::<UpdatedKey>(&other_version, PageRead::List).is_err());
        assert!(decode_cursor::<UpdatedKey>("not a cursor", PageRead::List).is_err());
    }

    #[test]
    fn a_page_starts_strictly_after_the_cursor_key() {
        let rows = [5, 4, 4, 3, 1];
        // Rows after a key of 4 in this descending order: 3 and 1.
        assert_eq!(page_start(&rows, |row| *row < 4), 3);
        assert_eq!(page_start(&rows, |_| true), 0);
        assert_eq!(page_start(&rows, |_| false), rows.len());
    }
}
