use crate::exec_bits::normalize_executable_bit;
use serde::de::{Error as DeError, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as ShaDigestTrait, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MobpackDigest([u8; 32]);

impl MobpackDigest {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for MobpackDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseDigestError(&'static str);

impl fmt::Display for ParseDigestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ParseDigestError {}

impl FromStr for MobpackDigest {
    type Err = ParseDigestError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 64 {
            return Err(ParseDigestError(
                "digest must be 64 lowercase hex characters",
            ));
        }
        let bytes =
            hex::decode(s).map_err(|_| ParseDigestError("digest must be valid lowercase hex"))?;
        let array: [u8; 32] = bytes
            .try_into()
            .map_err(|_| ParseDigestError("digest must decode to 32 bytes"))?;
        Ok(Self(array))
    }
}

impl Serialize for MobpackDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for MobpackDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MobpackDigestVisitor;

        impl Visitor<'_> for MobpackDigestVisitor {
            type Value = MobpackDigest;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a 64-byte lowercase hex digest")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                MobpackDigest::from_str(v).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(MobpackDigestVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalEntry {
    pub path: String,
    pub bytes: Vec<u8>,
}

pub fn sha256_bytes(input: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(input);
    hasher.finalize().into()
}

pub fn canonical_digest(entries: &[CanonicalEntry]) -> MobpackDigest {
    let mut index: Vec<(String, [u8; 32], bool)> = entries
        .iter()
        .map(|entry| {
            (
                normalize_path(&entry.path),
                sha256_bytes(&entry.bytes),
                normalize_executable_bit(&entry.path, &entry.bytes),
            )
        })
        .filter(|(path, _, _)| path != "signature.toml")
        .collect();

    index.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (path, file_digest, executable_bit) in index {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(hex::encode(file_digest).as_bytes());
        hasher.update([0]);
        hasher.update([if executable_bit { b'1' } else { b'0' }]);
        hasher.update([b'\n']);
    }

    MobpackDigest(hasher.finalize().into())
}

pub fn canonical_digest_from_map(files: &BTreeMap<String, Vec<u8>>) -> MobpackDigest {
    let entries: Vec<CanonicalEntry> = files
        .iter()
        .map(|(path, bytes)| CanonicalEntry {
            path: path.clone(),
            bytes: bytes.clone(),
        })
        .collect();
    canonical_digest(&entries)
}

fn normalize_path(path: &str) -> String {
    let replaced = path.replace('\\', "/");
    replaced
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_digest_display_fromstr_roundtrip() {
        let digest = MobpackDigest::from_bytes([0xabu8; 32]);
        let rendered = digest.to_string();
        let reparsed = MobpackDigest::from_str(&rendered).unwrap();
        assert_eq!(reparsed, digest);
    }

    #[test]
    fn test_sha256_deterministic() {
        let input = b"same input";
        assert_eq!(sha256_bytes(input), sha256_bytes(input));
    }

    #[test]
    fn test_canonical_digest_deterministic() {
        let mut a = HashMap::new();
        a.insert(
            "hooks/run.sh".to_string(),
            b"#!/bin/sh\necho run\n".to_vec(),
        );
        a.insert("skills/review.md".to_string(), b"review skill".to_vec());
        a.insert("config/defaults.toml".to_string(), b"timeout=30".to_vec());

        let mut b = HashMap::new();
        b.insert("config/defaults.toml".to_string(), b"timeout=30".to_vec());
        b.insert("skills/review.md".to_string(), b"review skill".to_vec());
        b.insert(
            "hooks/run.sh".to_string(),
            b"#!/bin/sh\necho run\n".to_vec(),
        );

        let entries_a: Vec<CanonicalEntry> = a
            .into_iter()
            .map(|(path, bytes)| CanonicalEntry { path, bytes })
            .collect();
        let entries_b: Vec<CanonicalEntry> = b
            .into_iter()
            .map(|(path, bytes)| CanonicalEntry { path, bytes })
            .collect();

        assert_eq!(canonical_digest(&entries_a), canonical_digest(&entries_b));
    }

    #[test]
    fn test_canonical_digest_excludes_signature() {
        let entries_without_signature = vec![
            CanonicalEntry {
                path: "manifest.toml".to_string(),
                bytes: b"[mobpack]\nname=\"x\"\nversion=\"1\"".to_vec(),
            },
            CanonicalEntry {
                path: "definition.json".to_string(),
                bytes: b"{\"id\":\"x\"}".to_vec(),
            },
        ];

        let mut entries_with_signature = entries_without_signature.clone();
        entries_with_signature.push(CanonicalEntry {
            path: "signature.toml".to_string(),
            bytes: b"signature = \"ignored\"".to_vec(),
        });

        assert_eq!(
            canonical_digest(&entries_without_signature),
            canonical_digest(&entries_with_signature)
        );
    }

    #[test]
    fn test_digest_from_normalized_content_is_platform_independent() {
        let windows_style = vec![
            CanonicalEntry {
                path: ".\\hooks\\run.sh".to_string(),
                bytes: b"#!/bin/sh\necho run\n".to_vec(),
            },
            CanonicalEntry {
                path: ".\\skills\\review.md".to_string(),
                bytes: b"# Review\n".to_vec(),
            },
            CanonicalEntry {
                path: "manifest.toml".to_string(),
                bytes: b"[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n".to_vec(),
            },
        ];

        let unix_style_reordered = vec![
            CanonicalEntry {
                path: "/manifest.toml".to_string(),
                bytes: b"[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n".to_vec(),
            },
            CanonicalEntry {
                path: "skills/review.md".to_string(),
                bytes: b"# Review\n".to_vec(),
            },
            CanonicalEntry {
                path: "./hooks/run.sh".to_string(),
                bytes: b"#!/bin/sh\necho run\n".to_vec(),
            },
        ];

        assert_eq!(
            canonical_digest(&windows_style),
            canonical_digest(&unix_style_reordered)
        );
    }
}
