//! Static observability for document-sized serialization and digest work.
//!
//! These counters describe cost only. They carry no persistence authority and
//! are deliberately separate from the released 0.8.10 proof importer.

use std::cell::Cell;

thread_local! {
    static CONTENT_DIGEST_COMPUTATIONS: Cell<u64> = const { Cell::new(0) };
    static CONTENT_DIGEST_BYTES: Cell<u64> = const { Cell::new(0) };
    static CURRENT_DIGEST_SITE: Cell<usize> = const { Cell::new(DIGEST_SITE_OTHER) };
}

#[doc(hidden)]
#[must_use]
pub fn session_content_digest_computations() -> u64 {
    CONTENT_DIGEST_COMPUTATIONS.with(Cell::get)
}

pub(crate) fn record_content_digest_computation() {
    CONTENT_DIGEST_COMPUTATIONS.with(|count| count.set(count.get().saturating_add(1)));
}

#[doc(hidden)]
#[must_use]
pub fn session_content_digest_bytes() -> u64 {
    CONTENT_DIGEST_BYTES.with(Cell::get)
}

pub(crate) fn record_content_digest_bytes(bytes: u64) {
    CONTENT_DIGEST_BYTES.with(|count| count.set(count.get().saturating_add(bytes)));
    GLOBAL_CONTENT_DIGEST_BYTES.fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
    DIGEST_SITE_BYTES[CURRENT_DIGEST_SITE.with(Cell::get)]
        .fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
}

static GLOBAL_CONTENT_DIGEST_BYTES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

#[doc(hidden)]
#[must_use]
pub fn global_session_content_digest_bytes() -> u64 {
    GLOBAL_CONTENT_DIGEST_BYTES.load(std::sync::atomic::Ordering::Relaxed)
}

static GLOBAL_SESSION_ENCODE_BYTES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

#[doc(hidden)]
pub fn record_session_encode_bytes(bytes: u64) {
    GLOBAL_SESSION_ENCODE_BYTES.fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
}

#[doc(hidden)]
#[must_use]
pub fn global_session_encode_bytes() -> u64 {
    GLOBAL_SESSION_ENCODE_BYTES.load(std::sync::atomic::Ordering::Relaxed)
}

static REWRITE_RECORD_BODY_DECODES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(crate) fn record_rewrite_record_body_decode() {
    REWRITE_RECORD_BODY_DECODES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

#[doc(hidden)]
#[must_use]
pub fn rewrite_record_body_decodes() -> u64 {
    REWRITE_RECORD_BODY_DECODES.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) const DIGEST_SITE_COUNT: usize = 6;
pub(crate) const DIGEST_SITE_OTHER: usize = 0;
pub(crate) const DIGEST_SITE_DECODE: usize = 1;
pub(crate) const DIGEST_SITE_ENCODE: usize = 2;
pub(crate) const DIGEST_SITE_REWRITE_CHAIN_WALK: usize = 3;
pub(crate) const DIGEST_SITE_APPEND_GUARD: usize = 4;
pub(crate) const DIGEST_SITE_BOUNDARY_GUARD: usize = 5;

#[doc(hidden)]
pub const DIGEST_SITE_LABELS: [&str; DIGEST_SITE_COUNT] = [
    "other",
    "decode",
    "encode",
    "rewrite-chain-walk",
    "append-guard",
    "boundary-guard",
];

static DIGEST_SITE_BYTES: [std::sync::atomic::AtomicU64; DIGEST_SITE_COUNT] =
    [const { std::sync::atomic::AtomicU64::new(0) }; DIGEST_SITE_COUNT];

#[doc(hidden)]
#[must_use]
pub fn digest_site_bytes() -> [u64; DIGEST_SITE_COUNT] {
    std::array::from_fn(|site| DIGEST_SITE_BYTES[site].load(std::sync::atomic::Ordering::Relaxed))
}

pub(crate) struct DigestSiteScope(usize);

impl Drop for DigestSiteScope {
    fn drop(&mut self) {
        CURRENT_DIGEST_SITE.with(|site| site.set(self.0));
    }
}

pub(crate) fn enter_digest_site(site: usize) -> DigestSiteScope {
    CURRENT_DIGEST_SITE.with(|current| {
        let enclosing = current.get();
        current.set(site);
        DigestSiteScope(enclosing)
    })
}

pub(crate) fn write_canonical_json(
    value: &serde_json::Value,
    output: &mut Vec<u8>,
) -> Result<(), serde_json::Error> {
    match value {
        serde_json::Value::Null => output.extend_from_slice(b"null"),
        serde_json::Value::Bool(value) => {
            output.extend_from_slice(if *value { b"true" } else { b"false" });
        }
        serde_json::Value::Number(value) => output.extend_from_slice(value.to_string().as_bytes()),
        serde_json::Value::String(value) => {
            output.extend_from_slice(serde_json::to_string(value)?.as_bytes());
        }
        serde_json::Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(b']');
        }
        serde_json::Value::Object(values) => {
            output.push(b'{');
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                output.extend_from_slice(serde_json::to_string(key)?.as_bytes());
                output.push(b':');
                write_canonical_json(value, output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}
