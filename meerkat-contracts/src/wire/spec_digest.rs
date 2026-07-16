//! Canonical digest over [`PortableMemberSpec`] (A12 — one digest owner).
//!
//! The mechanics deliberately copy the comms envelope-signature
//! canonicalization (`meerkat-comms` `Envelope::signable_bytes`): serde →
//! `ciborium::Value`, recursive RFC 8949 canonical map ordering (text keys
//! compare length-first then lexicographically; non-text keys fall back to
//! `CanonicalValue` ordering), deterministic CBOR encoding, SHA-256,
//! lowercase hex. Any change to this function is a wire-visible change: the
//! digest is the materialize idempotency co-key (`SpecDigestMismatch`).

use ciborium::value::{CanonicalValue, Value};
use sha2::{Digest, Sha256};

use super::portable_spec::PortableMemberSpec;

/// Typed failure computing a portable-member-spec digest.
#[derive(Debug, thiserror::Error)]
pub enum SpecDigestError {
    #[error("portable member spec could not be represented as CBOR: {detail}")]
    Represent { detail: String },
    #[error("portable member spec could not be CBOR-encoded: {detail}")]
    Encode { detail: String },
}

/// Recursively sort every CBOR map into RFC 8949 canonical key order.
///
/// `ciborium::into_writer` does not sort map keys, so ordering must be
/// normalized on the `Value` tree first — the exact comparator used for
/// comms envelope signing.
///
/// TWIN ALGORITHM: `meerkat-comms/src/types.rs` `Envelope::signable_bytes`
/// carries a byte-identical copy of this canonicalizer (the crates cannot
/// share code without a dependency cycle). Any RFC 8949 edge-case fix must
/// land in BOTH; the known-answer vector test below pins this copy's
/// semantics so divergence is loud.
fn canonicalize(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                canonicalize(item);
            }
        }
        Value::Map(entries) => {
            for (key, val) in entries.iter_mut() {
                canonicalize(key);
                canonicalize(val);
            }
            entries.sort_by(|(k1, _), (k2, _)| match (k1, k2) {
                (Value::Text(a), Value::Text(b)) => match a.len().cmp(&b.len()) {
                    std::cmp::Ordering::Equal => a.cmp(b),
                    ord => ord,
                },
                _ => CanonicalValue::from(k1.clone()).cmp(&CanonicalValue::from(k2.clone())),
            });
        }
        Value::Tag(_, inner) => canonicalize(inner),
        _ => {}
    }
}

/// Compute the canonical SHA-256 digest (lowercase hex) of a spec.
pub fn portable_member_spec_digest(spec: &PortableMemberSpec) -> Result<String, SpecDigestError> {
    let mut value = Value::serialized(spec).map_err(|error| SpecDigestError::Represent {
        detail: error.to_string(),
    })?;
    canonicalize(&mut value);
    let mut buf = Vec::new();
    ciborium::into_writer(&value, &mut buf).map_err(|error| SpecDigestError::Encode {
        detail: error.to_string(),
    })?;
    let digest = Sha256::digest(&buf);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        // Writing to a String cannot fail; the Result is structural.
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::super::portable_spec::sample_portable_member_spec;
    use super::*;

    /// Frozen regression pin for the canonicalization mechanics. If this
    /// digest changes, the encoding changed and every recorded
    /// `spec_digest` in host stores silently mismatches — that is a
    /// protocol event, not a test to update casually.
    ///
    /// The constant is seeded from the first verified run: an empty pin
    /// fails with the actual value so it can be recorded once.
    const FIXTURE_DIGEST_PIN: &str =
        "54c78bdb185b66b218d38e2f23f08055ceb9db8e6dd79dcb0dc0f7966b089e6b";

    #[test]
    fn digest_is_deterministic_across_calls() {
        let spec = sample_portable_member_spec();
        let first = portable_member_spec_digest(&spec).expect("digest");
        let second = portable_member_spec_digest(&spec).expect("digest");
        assert_eq!(first, second);
    }

    #[test]
    fn digest_is_lowercase_hex_sha256() {
        let digest = portable_member_spec_digest(&sample_portable_member_spec()).expect("digest");
        assert_eq!(digest.len(), 64, "SHA-256 hex digest must be 64 chars");
        assert!(
            digest
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "digest must be lowercase hex: {digest}"
        );
    }

    #[test]
    fn digest_is_stable_across_serialize_deserialize_round_trip() {
        let spec = sample_portable_member_spec();
        let original = portable_member_spec_digest(&spec).expect("digest");
        let json = serde_json::to_value(&spec).expect("serialize spec");
        let rehydrated: super::super::portable_spec::PortableMemberSpec =
            serde_json::from_value(json).expect("decode spec");
        let redigest = portable_member_spec_digest(&rehydrated).expect("digest");
        assert_eq!(
            original, redigest,
            "a wire round-trip must not change the canonical digest"
        );
    }

    #[test]
    fn digest_changes_when_any_field_changes() {
        let baseline = portable_member_spec_digest(&sample_portable_member_spec()).expect("digest");

        let mut identity_flip = sample_portable_member_spec();
        identity_flip.agent_identity = "worker-2".to_string();
        assert_ne!(
            baseline,
            portable_member_spec_digest(&identity_flip).expect("digest"),
            "identity flip must change the digest"
        );

        let mut nested_flip = sample_portable_member_spec();
        nested_flip.profile.tools.shell = true;
        assert_ne!(
            baseline,
            portable_member_spec_digest(&nested_flip).expect("digest"),
            "nested tool-toggle flip must change the digest"
        );
    }

    #[test]
    fn digest_matches_frozen_fixture_pin() {
        let digest = portable_member_spec_digest(&sample_portable_member_spec()).expect("digest");
        assert_eq!(
            digest, FIXTURE_DIGEST_PIN,
            "seed FIXTURE_DIGEST_PIN with this verified value on first run: {digest}"
        );
    }

    /// Known-answer vector for the RFC 8949 canonicalization semantics this
    /// canonicalizer must share byte-for-byte with the comms envelope twin
    /// (meerkat-comms/src/types.rs signable_bytes): text map keys sort
    /// length-first then lexicographically, nested maps and maps inside
    /// arrays/tags are sorted too. If this pin moves, the envelope twin must
    /// move in the same change-set.
    #[test]
    fn canonicalization_known_answer_vector() {
        use ciborium::value::Value;
        let mut value = Value::Map(vec![
            (
                Value::Text("bb".into()),
                Value::Map(vec![
                    (Value::Text("z".into()), Value::Integer(1.into())),
                    (Value::Text("a".into()), Value::Integer(2.into())),
                ]),
            ),
            (Value::Text("a".into()), Value::Integer(3.into())),
            (
                Value::Text("ab".into()),
                Value::Array(vec![Value::Map(vec![
                    (Value::Text("longer".into()), Value::Integer(4.into())),
                    (Value::Text("xy".into()), Value::Integer(5.into())),
                ])]),
            ),
        ]);
        super::canonicalize(&mut value);
        let mut buf = Vec::new();
        ciborium::into_writer(&value, &mut buf).expect("encode");
        // a3            map(3)
        //   61 61       "a"            -> 03
        //   62 61 62    "ab"           -> [ {"xy": 05, "longer": 04} ]
        //   62 62 62    "bb"           -> {"a": 02, "z": 01}
        let expected: &[u8] = &[
            0xa3, // map(3)
            0x61, b'a', 0x03, // "a": 3
            0x62, b'a', b'b', // "ab":
            0x81, // array(1)
            0xa2, // map(2)
            0x62, b'x', b'y', 0x05, // "xy": 5 (len 2 sorts before len 6)
            0x66, b'l', b'o', b'n', b'g', b'e', b'r', 0x04, // "longer": 4
            0x62, b'b', b'b', // "bb":
            0xa2, // map(2)
            0x61, b'a', 0x02, // "a": 2
            0x61, b'z', 0x01, // "z": 1
        ];
        assert_eq!(
            buf, expected,
            "canonical CBOR byte-domain drifted — fix BOTH canonicalizer twins"
        );
    }
}
